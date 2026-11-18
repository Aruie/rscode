//! An open text document: text, undo/redo history and file metadata.

use crate::text_ops::{Edit, Sel};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

const UNDO_LIMIT: usize = 2000;
const MERGE_WINDOW: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Where {
    Local(std::path::PathBuf),
    Remote(String),
    Untitled(usize),
}

impl Where {
    pub fn path_string(&self) -> String {
        match self {
            Where::Local(p) => p.to_string_lossy().into_owned(),
            Where::Remote(p) => p.clone(),
            Where::Untitled(n) => format!("Untitled-{n}"),
        }
    }
    pub fn file_name(&self) -> String {
        match self {
            Where::Local(p) => p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            Where::Remote(p) => p.rsplit('/').next().unwrap_or(p).to_string(),
            Where::Untitled(n) => format!("Untitled-{n}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Insert,
    Delete,
    Other,
}

struct Change {
    start: usize,
    removed: String,
    inserted: String,
    before: Sel,
    after: Sel,
    kind: Kind,
    at: Instant,
}

pub struct Doc {
    pub loc: Where,
    pub text: String,
    pub crlf: bool,
    pub use_tabs: bool,
    pub tab_size: usize,
    pub lang: &'static crate::lang::Lang,
    /// tree-sitter grammar for coloring, if one is installed for this file type.
    pub grammar: Option<&'static crate::syntax::Grammar>,
    /// Highlight spans for `text` (may lag one edit behind until the next highlight pass).
    pub spans: Vec<crate::syntax::Span>,
    pub read_only: bool,
    /// Shown instead of the editor (binary file, loading, error).
    pub message: String,
    saved_hash: u64,
    undo: Vec<Change>,
    redo: Vec<Change>,
    /// Last known selection, used as the "before" state of the next edit.
    pub sel: Sel,
}

pub fn hash_text(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl Doc {
    pub fn new(loc: Where, raw: String) -> Self {
        let crlf = raw.contains("\r\n");
        let text = if crlf { raw.replace("\r\n", "\n") } else { raw };
        let use_tabs = detect_tabs(&text);
        let lang = crate::lang::detect(&loc.file_name());
        let saved_hash = hash_text(&text);
        let mut d = Doc {
            loc,
            text,
            crlf,
            use_tabs,
            tab_size: 4,
            lang,
            grammar: None,
            spans: Vec::new(),
            read_only: false,
            message: String::new(),
            saved_hash,
            undo: Vec::new(),
            redo: Vec::new(),
            sel: Sel::default(),
        };
        if !d.loc.file_name().is_empty() {
            d.detect_grammar();
        }
        d
    }

    pub fn placeholder(loc: Where, message: &str) -> Self {
        let mut d = Doc::new(loc, String::new());
        d.read_only = true;
        d.message = message.to_string();
        d
    }

    /// Pick the grammar from the file name, falling back to the language mode's name.
    pub fn detect_grammar(&mut self) {
        let reg = crate::syntax::registry();
        let by_name = reg.by_file_name(&self.loc.file_name());
        self.grammar = by_name.or_else(|| reg.by_lang_name(self.lang.name));
        self.spans.clear();
    }

    pub fn lang_name(&self) -> &str {
        match self.grammar {
            Some(g) if std::ptr::eq(self.lang, &crate::lang::PLAIN) => &g.name,
            _ => self.lang.name,
        }
    }

    pub fn comment_token(&self) -> &str {
        match self.grammar {
            Some(g) if self.lang.comment.is_empty() => &g.comment,
            _ => self.lang.comment,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.message.is_empty() && hash_text(&self.text) != self.saved_hash
    }

    pub fn mark_saved(&mut self, hash: u64) {
        self.saved_hash = hash;
    }

    pub fn current_hash(&self) -> u64 {
        hash_text(&self.text)
    }

    pub fn bytes_for_save(&self) -> Vec<u8> {
        if self.crlf { self.text.replace('\n', "\r\n").into_bytes() } else { self.text.clone().into_bytes() }
    }

    pub fn indent_unit(&self) -> String {
        if self.use_tabs { "\t".into() } else { " ".repeat(self.tab_size) }
    }

    /// Record a change made by the text widget itself (typing, paste, cut...).
    /// The widget's new text is diffed against ours.
    pub fn sync_from_widget(&mut self, new_text: &str, after: Sel) {
        if new_text == self.text {
            self.sel = after;
            return;
        }
        let old = self.text.as_bytes();
        let new = new_text.as_bytes();
        let mut pre = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        while !self.text.is_char_boundary(pre) || !new_text.is_char_boundary(pre) {
            pre -= 1;
        }
        let max_suf = old.len().min(new.len()) - pre;
        let mut suf = old.iter().rev().zip(new.iter().rev()).take(max_suf).take_while(|(a, b)| a == b).count();
        while !self.text.is_char_boundary(old.len() - suf) || !new_text.is_char_boundary(new.len() - suf) {
            suf -= 1;
        }
        let removed = self.text[pre..old.len() - suf].to_string();
        let inserted = new_text[pre..new.len() - suf].to_string();
        let kind = if removed.is_empty() {
            Kind::Insert
        } else if inserted.is_empty() {
            Kind::Delete
        } else {
            Kind::Other
        };
        let before = self.sel;
        self.text = new_text.to_string();
        self.push(Change { start: pre, removed, inserted, before, after, kind, at: Instant::now() }, true);
        self.sel = after;
    }

    /// Apply one of our own edits (shortcuts).
    pub fn apply(&mut self, e: &Edit) {
        let removed = self.text[e.start..e.end].to_string();
        let before = self.sel;
        e.apply(&mut self.text);
        self.push(
            Change { start: e.start, removed, inserted: e.insert.clone(), before, after: e.sel, kind: Kind::Other, at: Instant::now() },
            false,
        );
        self.sel = e.sel;
    }

    fn push(&mut self, c: Change, mergeable: bool) {
        self.redo.clear();
        if mergeable {
            if let Some(last) = self.undo.last_mut() {
                let recent = c.at.duration_since(last.at) < MERGE_WINDOW;
                let word_break = c.inserted.contains('\n') || (last.inserted.ends_with(' ') && !c.inserted.starts_with(' '));
                if recent && last.kind == c.kind && !word_break {
                    if c.kind == Kind::Insert && c.start == last.start + last.inserted.len() {
                        last.inserted.push_str(&c.inserted);
                        last.after = c.after;
                        last.at = c.at;
                        return;
                    }
                    if c.kind == Kind::Delete && c.start + c.removed.len() == last.start {
                        // backspace run
                        last.removed.insert_str(0, &c.removed);
                        last.start = c.start;
                        last.after = c.after;
                        last.at = c.at;
                        return;
                    }
                    if c.kind == Kind::Delete && c.start == last.start {
                        // forward-delete run
                        last.removed.push_str(&c.removed);
                        last.after = c.after;
                        last.at = c.at;
                        return;
                    }
                }
            }
        }
        self.undo.push(c);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Stop merging the next typed characters into the previous undo step (cursor moved, etc).
    pub fn break_merge(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.kind = Kind::Other;
        }
    }

    pub fn undo(&mut self) -> Option<Sel> {
        let c = self.undo.pop()?;
        self.text.replace_range(c.start..c.start + c.inserted.len(), &c.removed);
        self.sel = c.before;
        let s = c.before;
        self.redo.push(c);
        Some(s)
    }

    pub fn redo(&mut self) -> Option<Sel> {
        let c = self.redo.pop()?;
        self.text.replace_range(c.start..c.start + c.removed.len(), &c.inserted);
        self.sel = c.after;
        let s = c.after;
        self.undo.push(c);
        Some(s)
    }
}

fn detect_tabs(text: &str) -> bool {
    let (mut tabs, mut spaces) = (0, 0);
    for line in text.lines().take(2000) {
        if line.starts_with('\t') {
            tabs += 1;
        } else if line.starts_with("  ") {
            spaces += 1;
        }
    }
    tabs > spaces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_merges_and_undoes() {
        let mut d = Doc::new(Where::Untitled(1), String::new());
        d.sync_from_widget("a", Sel::caret(1));
        d.sync_from_widget("ab", Sel::caret(2));
        d.sync_from_widget("ab ", Sel::caret(3));
        d.sync_from_widget("ab c", Sel::caret(4));
        assert!(d.is_dirty());
        assert_eq!(d.undo(), Some(Sel::caret(3)));
        assert_eq!(d.text, "ab ");
        assert_eq!(d.undo(), Some(Sel::caret(0)));
        assert_eq!(d.text, "");
        assert!(!d.is_dirty());
        d.redo();
        d.redo();
        assert_eq!(d.text, "ab c");
    }

    #[test]
    fn multibyte_diff() {
        let mut d = Doc::new(Where::Untitled(1), "한글".into());
        d.sync_from_widget("한국글", Sel::caret(6));
        d.undo();
        assert_eq!(d.text, "한글");
    }

    #[test]
    fn crlf_roundtrip() {
        let d = Doc::new(Where::Untitled(1), "a\r\nb".into());
        assert_eq!(d.text, "a\nb");
        assert_eq!(d.bytes_for_save(), b"a\r\nb");
    }
}
