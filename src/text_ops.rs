//! Pure text operations behind the VS Code editing shortcuts.
//! Every operation returns a single range replacement plus the new selection,
//! so the undo stack can record it like any typed edit.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Sel {
    pub anchor: usize,
    pub cursor: usize,
}

impl Sel {
    pub fn caret(p: usize) -> Self {
        Sel { anchor: p, cursor: p }
    }
    pub fn new(anchor: usize, cursor: usize) -> Self {
        Sel { anchor, cursor }
    }
    pub fn start(&self) -> usize {
        self.anchor.min(self.cursor)
    }
    pub fn end(&self) -> usize {
        self.anchor.max(self.cursor)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
    pub sel: Sel,
}

impl Edit {
    pub fn apply(&self, text: &mut String) {
        text.replace_range(self.start..self.end, &self.insert);
    }
}

pub fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

pub fn line_end(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map(|i| i + pos).unwrap_or(text.len())
}

pub fn line_index(text: &str, pos: usize) -> usize {
    text.as_bytes()[..pos].iter().filter(|&&b| b == b'\n').count()
}

/// Byte offset of the start of line `idx` (0-based), clamped to the last line.
pub fn line_offset(text: &str, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    let mut n = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            n += 1;
            if n == idx {
                return i + 1;
            }
        }
    }
    line_start(text, text.len())
}

fn leading_ws(line: &str) -> &str {
    let n = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[..n]
}

/// Visual column (tabs expand to `tab` columns).
fn visual_width(s: &str, tab: usize) -> usize {
    s.chars().fold(0, |c, ch| if ch == '\t' { c + tab - c % tab } else { c + 1 })
}

/// The block of whole lines touched by a selection: (start of first line, end of last line).
/// Like VS Code, a selection ending at column 0 of a line does not include that line.
pub fn selected_lines(text: &str, sel: Sel) -> (usize, usize) {
    let s = sel.start();
    let mut e = sel.end();
    if !sel.is_empty() && e > s && line_start(text, e) == e {
        e -= 1;
    }
    (line_start(text, s), line_end(text, e))
}

/// Per-line edits inside one block; positions are mapped through them.
struct LineEdit {
    pos: usize,
    del: usize,
    ins: String,
}

fn apply_line_edits(text: &str, block: (usize, usize), edits: &[LineEdit], sel: Sel) -> Edit {
    let mut out = String::with_capacity(block.1 - block.0 + edits.len() * 4);
    let mut last = block.0;
    for e in edits {
        out.push_str(&text[last..e.pos]);
        out.push_str(&e.ins);
        last = e.pos + e.del;
    }
    out.push_str(&text[last..block.1]);
    // `stay`: the start of a non-empty selection sitting exactly at an insertion point
    // stays before the inserted text (so the selection grows to include it).
    let map = |p: usize, stay: bool| -> usize {
        let mut q = p as isize;
        for e in edits {
            if p < e.pos || (p == e.pos && (e.del > 0 || stay)) {
                break;
            }
            if p < e.pos + e.del {
                // inside deleted text: collapse to the edit position
                q += e.pos as isize - p as isize;
                break;
            }
            q += e.ins.len() as isize - e.del as isize;
        }
        q.max(0) as usize
    };
    let anchor_is_start = sel.anchor <= sel.cursor;
    let nonempty = !sel.is_empty();
    Edit {
        start: block.0,
        end: block.1,
        insert: out,
        sel: Sel::new(map(sel.anchor, nonempty && anchor_is_start), map(sel.cursor, nonempty && !anchor_is_start)),
    }
}

fn line_starts_in(text: &str, block: (usize, usize)) -> Vec<usize> {
    let mut v = vec![block.0];
    for (i, b) in text[block.0..block.1].bytes().enumerate() {
        if b == b'\n' {
            v.push(block.0 + i + 1);
        }
    }
    v
}

pub fn indent_lines(text: &str, sel: Sel, unit: &str) -> Edit {
    let block = selected_lines(text, sel);
    let edits: Vec<LineEdit> = line_starts_in(text, block)
        .into_iter()
        .filter(|&ls| {
            let le = line_end(text, ls);
            le > ls || sel.is_empty()
        })
        .map(|ls| LineEdit { pos: ls, del: 0, ins: unit.to_string() })
        .collect();
    apply_line_edits(text, block, &edits, sel)
}

pub fn outdent_lines(text: &str, sel: Sel, tab: usize) -> Edit {
    let block = selected_lines(text, sel);
    let mut edits = Vec::new();
    for ls in line_starts_in(text, block) {
        let line = &text[ls..line_end(text, ls)];
        let del = if line.starts_with('\t') {
            1
        } else {
            line.bytes().take(tab).take_while(|&b| b == b' ').count()
        };
        if del > 0 {
            edits.push(LineEdit { pos: ls, del, ins: String::new() });
        }
    }
    apply_line_edits(text, block, &edits, sel)
}

pub fn toggle_line_comment(text: &str, sel: Sel, token: &str) -> Option<Edit> {
    if token.is_empty() {
        return None;
    }
    let block = selected_lines(text, sel);
    let starts = line_starts_in(text, block);
    let lines: Vec<(usize, &str)> = starts.iter().map(|&ls| (ls, &text[ls..line_end(text, ls)])).collect();
    let non_empty: Vec<&(usize, &str)> = lines.iter().filter(|(_, l)| !l.trim().is_empty()).collect();
    if non_empty.is_empty() {
        return None;
    }
    let all_commented = non_empty.iter().all(|(_, l)| l.trim_start().starts_with(token));
    let mut edits = Vec::new();
    if all_commented {
        for (ls, l) in non_empty {
            let ws = leading_ws(l).len();
            let mut del = token.len();
            if l[ws + del..].starts_with(' ') {
                del += 1;
            }
            edits.push(LineEdit { pos: ls + ws, del, ins: String::new() });
        }
    } else {
        let min_ws = non_empty.iter().map(|(_, l)| leading_ws(l).len()).min().unwrap_or(0);
        for (ls, _) in non_empty {
            edits.push(LineEdit { pos: ls + min_ws, del: 0, ins: format!("{token} ") });
        }
    }
    Some(apply_line_edits(text, block, &edits, sel))
}

pub fn move_lines(text: &str, sel: Sel, up: bool) -> Option<Edit> {
    let (bs, be) = selected_lines(text, sel);
    let block = &text[bs..be];
    if up {
        if bs == 0 {
            return None;
        }
        let ps = line_start(text, bs - 1);
        let prev = &text[ps..bs - 1];
        let insert = format!("{block}\n{prev}");
        let shift = prev.len() + 1;
        Some(Edit { start: ps, end: be, insert, sel: Sel::new(sel.anchor - shift, sel.cursor - shift) })
    } else {
        if be >= text.len() {
            return None;
        }
        let ne = line_end(text, be + 1);
        let next = &text[be + 1..ne];
        let insert = format!("{next}\n{block}");
        let shift = next.len() + 1;
        Some(Edit { start: bs, end: ne, insert, sel: Sel::new(sel.anchor + shift, sel.cursor + shift) })
    }
}

pub fn copy_lines(text: &str, sel: Sel, up: bool) -> Edit {
    let (bs, be) = selected_lines(text, sel);
    let block = &text[bs..be];
    if up {
        Edit { start: bs, end: bs, insert: format!("{block}\n"), sel }
    } else {
        let shift = block.len() + 1;
        Edit { start: be, end: be, insert: format!("\n{block}"), sel: Sel::new(sel.anchor + shift, sel.cursor + shift) }
    }
}

pub fn delete_lines(text: &str, sel: Sel) -> Edit {
    let (bs, be) = selected_lines(text, sel);
    let col = sel.cursor - line_start(text, sel.cursor);
    let (start, end) = if be < text.len() {
        (bs, be + 1)
    } else if bs > 0 {
        (bs - 1, be)
    } else {
        (bs, be)
    };
    let mut rest = String::with_capacity(text.len());
    rest.push_str(&text[..start]);
    rest.push_str(&text[end..]);
    let target_ls = if be < text.len() { bs } else { line_start(&rest, start) };
    let target_le = line_end(&rest, target_ls);
    let caret = floor_char(&rest, (target_ls + col).min(target_le));
    Edit { start, end, insert: String::new(), sel: Sel::caret(caret) }
}

pub fn insert_line(text: &str, sel: Sel, above: bool) -> Edit {
    let ls = line_start(text, sel.cursor);
    let le = line_end(text, sel.cursor);
    let indent = leading_ws(&text[ls..le]).to_string();
    if above {
        let caret = ls + indent.len();
        Edit { start: ls, end: ls, insert: format!("{indent}\n"), sel: Sel::caret(caret) }
    } else {
        let caret = le + 1 + indent.len();
        Edit { start: le, end: le, insert: format!("\n{indent}"), sel: Sel::caret(caret) }
    }
}

fn closing_for(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

pub fn newline(text: &str, sel: Sel, unit: &str) -> Edit {
    let (s, e) = (sel.start(), sel.end());
    let ls = line_start(text, s);
    let mut indent = leading_ws(&text[ls..s]).to_string();
    let before = text[ls..s].trim_end_matches([' ', '\t']);
    let after_char = text[e..].chars().next();
    let opener = before.chars().last();
    let increase = matches!(opener, Some('{') | Some('(') | Some('[') | Some(':'));
    if increase {
        let inner = format!("{indent}{unit}");
        if let (Some(o), Some(a)) = (opener, after_char) {
            if closing_for(o) == Some(a) {
                let insert = format!("\n{inner}\n{indent}");
                let caret = s + 1 + inner.len();
                return Edit { start: s, end: e, insert, sel: Sel::caret(caret) };
            }
        }
        indent = inner;
    }
    let insert = format!("\n{indent}");
    let caret = s + insert.len();
    Edit { start: s, end: e, insert, sel: Sel::caret(caret) }
}

/// Tab without a multi-line selection: insert spaces up to the next tab stop.
pub fn tab_insert(text: &str, sel: Sel, use_tabs: bool, tab: usize) -> Edit {
    let s = sel.start();
    let ls = line_start(text, s);
    let insert = if use_tabs {
        "\t".to_string()
    } else {
        let col = visual_width(&text[ls..s], tab);
        " ".repeat(tab - col % tab)
    };
    let caret = s + insert.len();
    Edit { start: s, end: sel.end(), insert, sel: Sel::caret(caret) }
}

/// Auto-closing pairs and surrounding a selection, like VS Code's defaults.
pub fn type_char(text: &str, sel: Sel, c: char) -> Option<Edit> {
    let (s, e) = (sel.start(), sel.end());
    let next = text[e..].chars().next();
    let prev = text[..s].chars().last();
    let close = match c {
        '(' | '[' | '{' => closing_for(c),
        '"' | '\'' | '`' => Some(c),
        _ => None,
    };
    // overtype a closing character that is already there
    if sel.is_empty() && matches!(c, ')' | ']' | '}' | '"' | '\'' | '`') && next == Some(c) {
        return Some(Edit { start: s, end: s, insert: String::new(), sel: Sel::caret(s + c.len_utf8()) });
    }
    let close = close?;
    if !sel.is_empty() {
        let inner = &text[s..e];
        let insert = format!("{c}{inner}{close}");
        return Some(Edit { start: s, end: e, insert, sel: Sel::new(s + 1, s + 1 + inner.len()) });
    }
    let next_ok = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ',' | ';' | ':' | '.'));
    if !next_ok {
        return None;
    }
    if c == '\'' || c == '"' || c == '`' {
        // don't pair after a word character (it's an apostrophe) or inside a word
        if prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || p == c) {
            return None;
        }
    }
    Some(Edit { start: s, end: s, insert: format!("{c}{close}"), sel: Sel::caret(s + c.len_utf8()) })
}

/// Backspace: delete an empty auto-closed pair, or delete back to the previous tab stop in indentation.
pub fn backspace(text: &str, sel: Sel, tab: usize) -> Option<Edit> {
    if !sel.is_empty() || sel.cursor == 0 {
        return None;
    }
    let p = sel.cursor;
    let prev = text[..p].chars().last()?;
    let next = text[p..].chars().next();
    let pair = match prev {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        '`' => Some('`'),
        _ => None,
    };
    if pair.is_some() && pair == next {
        return Some(Edit { start: p - 1, end: p + 1, insert: String::new(), sel: Sel::caret(p - 1) });
    }
    let ls = line_start(text, p);
    let before = &text[ls..p];
    if !before.is_empty() && before.bytes().all(|b| b == b' ') {
        let n = before.len();
        let del = if n % tab == 0 { tab } else { n % tab };
        return Some(Edit { start: p - del, end: p, insert: String::new(), sel: Sel::caret(p - del) });
    }
    None
}

/// Home: toggle between first non-whitespace character and column 0.
pub fn smart_home(text: &str, sel: Sel, extend: bool) -> Sel {
    let ls = line_start(text, sel.cursor);
    let le = line_end(text, sel.cursor);
    let first = ls + leading_ws(&text[ls..le]).len();
    let target = if sel.cursor == first { ls } else { first };
    if extend { Sel::new(sel.anchor, target) } else { Sel::caret(target) }
}

pub fn select_line(text: &str, sel: Sel) -> Sel {
    let (bs, be) = selected_lines(text, sel);
    let already = !sel.is_empty() && sel.start() == bs && (sel.end() == be + 1 || sel.end() == text.len());
    let (bs, be) = if already && sel.end() < text.len() {
        (bs, line_end(text, sel.end()))
    } else {
        (bs, be)
    };
    Sel::new(bs, (be + 1).min(text.len()))
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn word_at(text: &str, pos: usize) -> (usize, usize) {
    let mut s = pos;
    for (i, c) in text[..pos].char_indices().rev() {
        if !is_word(c) {
            break;
        }
        s = i;
    }
    let mut e = pos;
    for (i, c) in text[pos..].char_indices() {
        if !is_word(c) {
            break;
        }
        e = pos + i + c.len_utf8();
    }
    (s, e)
}

pub fn floor_char(text: &str, mut p: usize) -> usize {
    p = p.min(text.len());
    while !text.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// All matches (byte ranges). Case-insensitive matching folds ASCII only, which keeps byte offsets intact.
pub fn find_all(text: &str, needle: &str, case: bool, word: bool, limit: usize) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let (hay, nee);
    let (h, n): (&str, &str) = if case {
        (text, needle)
    } else {
        hay = text.to_ascii_lowercase();
        nee = needle.to_ascii_lowercase();
        (&hay, &nee)
    };
    let mut out = Vec::new();
    for (i, _) in h.match_indices(n) {
        let e = i + n.len();
        if word {
            let b = text[..i].chars().last().is_some_and(is_word);
            let a = text[e..].chars().next().is_some_and(is_word);
            if a || b {
                continue;
            }
        }
        out.push((i, e));
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// (line, column) as VS Code reports them: 1-based, columns count characters (tabs expand).
pub fn line_col(text: &str, pos: usize, tab: usize) -> (usize, usize) {
    let pos = floor_char(text, pos);
    let ls = line_start(text, pos);
    (line_index(text, pos) + 1, visual_width(&text[ls..pos], tab) + 1)
}

/// Byte offset of (1-based line, 1-based char column).
pub fn offset_of(text: &str, line: usize, col: usize) -> usize {
    let ls = line_offset(text, line.saturating_sub(1));
    let le = line_end(text, ls);
    let mut p = ls;
    for (n, (i, c)) in text[ls..le].char_indices().enumerate() {
        if n + 1 >= col {
            return ls + i;
        }
        p = ls + i + c.len_utf8();
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, e: &Edit) -> String {
        let mut t = text.to_string();
        e.apply(&mut t);
        t
    }

    #[test]
    fn indent_and_outdent_keep_selection() {
        let t = "a\nb\nc";
        let e = indent_lines(t, Sel::new(0, 3), "    ");
        assert_eq!(run(t, &e), "    a\n    b\nc");
        assert_eq!(e.sel, Sel::new(0, 11));
        let t2 = run(t, &e);
        let o = outdent_lines(&t2, e.sel, 4);
        assert_eq!(run(&t2, &o), t);
    }

    #[test]
    fn comment_toggle() {
        let t = "  a\n  b";
        let e = toggle_line_comment(t, Sel::new(0, t.len()), "//").unwrap();
        let c = run(t, &e);
        assert_eq!(c, "  // a\n  // b");
        let e2 = toggle_line_comment(&c, Sel::new(0, c.len()), "//").unwrap();
        assert_eq!(run(&c, &e2), t);
    }

    #[test]
    fn move_and_copy() {
        let t = "one\ntwo\nthree";
        let e = move_lines(t, Sel::caret(5), true).unwrap();
        assert_eq!(run(t, &e), "two\none\nthree");
        assert_eq!(e.sel, Sel::caret(1));
        let e = move_lines(t, Sel::caret(1), false).unwrap();
        assert_eq!(run(t, &e), "two\none\nthree");
        assert_eq!(e.sel, Sel::caret(5));
        assert!(move_lines(t, Sel::caret(9), false).is_none());
        let e = copy_lines(t, Sel::caret(5), false);
        assert_eq!(run(t, &e), "one\ntwo\ntwo\nthree");
        assert_eq!(e.sel, Sel::caret(9));
    }

    #[test]
    fn delete_line_variants() {
        let t = "one\ntwo\nthree";
        let e = delete_lines(t, Sel::caret(5));
        assert_eq!(run(t, &e), "one\nthree");
        assert_eq!(e.sel, Sel::caret(5));
        let e = delete_lines(t, Sel::caret(10));
        assert_eq!(run(t, &e), "one\ntwo");
    }

    #[test]
    fn enter_indents_between_braces() {
        let t = "fn x() {}";
        let e = newline(t, Sel::caret(8), "    ");
        assert_eq!(run(t, &e), "fn x() {\n    \n}");
        assert_eq!(e.sel, Sel::caret(13));
        let t = "    let a = 1;";
        let e = newline(t, Sel::caret(t.len()), "    ");
        assert_eq!(run(t, &e), "    let a = 1;\n    ");
    }

    #[test]
    fn pairs() {
        let t = "x";
        let e = type_char(t, Sel::caret(1), '(').unwrap();
        assert_eq!(run(t, &e), "x()");
        let t = "()";
        let e = type_char(t, Sel::caret(1), ')').unwrap();
        assert_eq!(e.sel, Sel::caret(2));
        assert_eq!(run(t, &e), "()");
        let e = backspace(t, Sel::caret(1), 4).unwrap();
        assert_eq!(run(t, &e), "");
        assert!(type_char("don", Sel::caret(3), '\'').is_none());
        let e = type_char("ab", Sel::new(0, 2), '"').unwrap();
        assert_eq!(run("ab", &e), "\"ab\"");
    }

    #[test]
    fn tabs_and_backspace_tabstop() {
        let t = "  x";
        let e = tab_insert(t, Sel::caret(2), false, 4);
        assert_eq!(run(t, &e), "    x");
        let t = "      ";
        let e = backspace(t, Sel::caret(6), 4).unwrap();
        assert_eq!(run(t, &e), "    ");
    }

    #[test]
    fn home_and_lines() {
        let t = "    abc";
        assert_eq!(smart_home(t, Sel::caret(6), false), Sel::caret(4));
        assert_eq!(smart_home(t, Sel::caret(4), false), Sel::caret(0));
        assert_eq!(select_line("a\nb", Sel::caret(0)), Sel::new(0, 2));
        assert_eq!(line_col("a\n한글x", 8, 4), (2, 3));
        assert_eq!(offset_of("a\n한글x", 2, 3), 8);
    }

    #[test]
    fn find_options() {
        let t = "Foo foo food";
        assert_eq!(find_all(t, "foo", false, false, 100).len(), 3);
        assert_eq!(find_all(t, "foo", true, false, 100).len(), 2);
        assert_eq!(find_all(t, "foo", false, true, 100).len(), 2);
    }
}
