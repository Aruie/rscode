//! Syntax highlighting with tree-sitter grammars loaded at runtime from a `grammars/` folder,
//! so new languages can be added without rebuilding the editor.
//!
//! Layout (next to the executable, or in $RCODE_GRAMMARS):
//! ```text
//! grammars/python/config.txt     name = Python / extensions = py pyw / comment = #
//! grammars/python/parser.dll     (parser.so on Linux, parser.dylib on macOS)
//! grammars/python/highlights.scm
//! ```

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names we color, and their VS Code Dark Modern colors. The highlighter picks the
/// longest matching prefix, e.g. `keyword.return` falls back to `keyword`.
const NAMES: &[(&str, u32)] = &[
    ("attribute", 0xdcdcaa),
    ("boolean", 0x569cd6),
    ("character", 0xce9178),
    ("comment", 0x6a9955),
    ("conditional", 0xc586c0),
    ("constant", 0x4fc1ff),
    ("constant.builtin", 0x569cd6),
    ("constructor", 0x4ec9b0),
    ("embedded", 0x9cdcfe),
    ("escape", 0xd7ba7d),
    ("exception", 0xc586c0),
    ("field", 0x9cdcfe),
    ("function", 0xdcdcaa),
    ("function.builtin", 0xdcdcaa),
    ("function.macro", 0x569cd6),
    ("include", 0xc586c0),
    ("keyword", 0x569cd6),
    ("keyword.control", 0xc586c0),
    ("keyword.conditional", 0xc586c0),
    ("keyword.repeat", 0xc586c0),
    ("keyword.return", 0xc586c0),
    ("keyword.import", 0xc586c0),
    ("keyword.exception", 0xc586c0),
    ("label", 0xc8c8c8),
    ("method", 0xdcdcaa),
    ("module", 0x4ec9b0),
    ("namespace", 0x4ec9b0),
    ("number", 0xb5cea8),
    ("operator", 0xd4d4d4),
    ("parameter", 0x9cdcfe),
    ("property", 0x9cdcfe),
    ("punctuation", 0xcccccc),
    ("punctuation.special", 0x569cd6),
    ("repeat", 0xc586c0),
    ("string", 0xce9178),
    ("string.escape", 0xd7ba7d),
    ("string.regex", 0xd16969),
    ("string.special", 0xd7ba7d),
    ("tag", 0x569cd6),
    ("tag.attribute", 0x9cdcfe),
    ("text.literal", 0xce9178),
    ("text.title", 0x569cd6),
    ("type", 0x4ec9b0),
    ("type.builtin", 0x4ec9b0),
    ("variable", 0x9cdcfe),
    ("variable.builtin", 0x569cd6),
    ("variable.parameter", 0x9cdcfe),
];

pub const DEFAULT_FG: u32 = 0xcccccc;
/// Files bigger than this are shown without colors.
const MAX_HIGHLIGHT: usize = 2 * 1024 * 1024;

pub fn color_of(class: u8) -> u32 {
    NAMES.get(class as usize).map(|n| n.1).unwrap_or(DEFAULT_FG)
}

pub struct Grammar {
    pub name: String,
    pub aliases: Vec<String>,
    pub extensions: Vec<String>,
    pub file_names: Vec<String>,
    pub comment: String,
    dir: PathBuf,
    symbol: String,
    config: OnceLock<Option<HighlightConfiguration>>,
}

pub struct Registry {
    pub grammars: Vec<Grammar>,
}

fn grammars_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RCODE_GRAMMARS") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("grammars");
    dir.is_dir().then_some(dir)
}

fn parse_config(dir: &Path) -> Option<Grammar> {
    let text = std::fs::read_to_string(dir.join("config.txt")).ok()?;
    let folder = dir.file_name()?.to_string_lossy().into_owned();
    let mut g = Grammar {
        name: folder.clone(),
        aliases: Vec::new(),
        extensions: Vec::new(),
        file_names: Vec::new(),
        comment: String::new(),
        dir: dir.to_path_buf(),
        symbol: format!("tree_sitter_{}", folder.replace('-', "_")),
        config: OnceLock::new(),
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "name" => g.name = v.to_string(),
            "aliases" => g.aliases = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            "extensions" => g.extensions = v.split_whitespace().map(|s| s.trim_start_matches('.').to_ascii_lowercase()).collect(),
            "files" => g.file_names = v.split_whitespace().map(|s| s.to_ascii_lowercase()).collect(),
            "comment" => g.comment = v.to_string(),
            "symbol" => g.symbol = v.to_string(),
            _ => {}
        }
    }
    Some(g)
}

pub fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| {
        let dir = grammars_dir();
        let mut grammars: Vec<Grammar> = dir
            .as_ref()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| parse_config(&e.path())).collect())
            .unwrap_or_default();
        grammars.sort_by(|a, b| a.name.cmp(&b.name));
        Registry { grammars }
    })
}

static DISABLED: std::sync::RwLock<std::collections::BTreeSet<String>> = std::sync::RwLock::new(std::collections::BTreeSet::new());

/// Grammars (by name) turned off in the Extensions view.
pub fn set_disabled(names: std::collections::BTreeSet<String>) {
    *DISABLED.write().unwrap() = names;
}

fn enabled(g: &Grammar) -> bool {
    !DISABLED.read().unwrap().contains(&g.name)
}

impl Registry {
    pub fn by_file_name(&self, file_name: &str) -> Option<&Grammar> {
        let lower = file_name.to_ascii_lowercase();
        if let Some(g) = self.grammars.iter().find(|g| g.file_names.contains(&lower)) {
            return Some(g).filter(|g| enabled(g));
        }
        let ext = lower.rsplit_once('.')?.1;
        self.grammars.iter().find(|g| g.extensions.iter().any(|e| e == ext)).filter(|g| enabled(g))
    }

    pub fn by_lang_name(&self, name: &str) -> Option<&Grammar> {
        self.grammars.iter().find(|g| g.name == name || g.aliases.iter().any(|a| a == name)).filter(|g| enabled(g))
    }

    /// A code fence hint like "python", "py", "rust", "sh".
    pub fn by_hint(&self, hint: &str) -> Option<&Grammar> {
        let h = hint.trim().to_ascii_lowercase();
        if h.is_empty() {
            return None;
        }
        let alias = match h.as_str() {
            "shell" | "console" => "sh",
            "c++" => "cpp",
            "js" | "jsx" | "node" => "js",
            "golang" => "go",
            "yml" => "yaml",
            "csharp" | "c#" => "cs",
            other => other,
        };
        self.grammars
            .iter()
            .find(|g| g.name.to_ascii_lowercase() == h || g.extensions.iter().any(|e| e == alias))
            .filter(|g| enabled(g))
    }
}

fn lib_file(dir: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "parser.dll"
    } else if cfg!(target_os = "macos") {
        "parser.dylib"
    } else {
        "parser.so"
    };
    dir.join(name)
}

impl Grammar {
    /// Load the parser library and queries on first use. Errors are reported once and the
    /// language then shows without colors.
    pub fn config(&self) -> Result<&HighlightConfiguration, String> {
        let c = self.config.get_or_init(|| match self.load() {
            Ok(c) => Some(c),
            Err(e) => {
                LOAD_ERRORS.with(|l| l.borrow_mut().push(format!("{}: {e}", self.name)));
                None
            }
        });
        c.as_ref().ok_or_else(|| format!("The {} grammar could not be loaded.", self.name))
    }

    fn load(&self) -> Result<HighlightConfiguration, String> {
        let path = lib_file(&self.dir);
        // SAFETY: loading a grammar library runs no initialization code of note; the library is
        // kept loaded for the rest of the process (leaked) because the Language points into it.
        let language = unsafe {
            let lib = libloading::Library::new(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let lib: &'static libloading::Library = Box::leak(Box::new(lib));
            let f: libloading::Symbol<unsafe extern "C" fn() -> *const ()> =
                lib.get(self.symbol.as_bytes()).map_err(|e| format!("symbol {}: {e}", self.symbol))?;
            tree_sitter::Language::new(tree_sitter_language::LanguageFn::from_raw(*f))
        };
        let read = |n: &str| std::fs::read_to_string(self.dir.join(n)).unwrap_or_default();
        let highlights = read("highlights.scm");
        if highlights.is_empty() {
            return Err("highlights.scm is missing".into());
        }
        let mut cfg = HighlightConfiguration::new(language, &self.name, &highlights, &read("injections.scm"), &read("locals.scm"))
            .or_else(|_| {
                // Some upstream locals/injections queries use node types a grammar build lacks;
                // highlights alone are enough.
                let lang = cfg_language(&self.dir, &self.symbol)?;
                HighlightConfiguration::new(lang, &self.name, &highlights, "", "")
            })
            .map_err(|e| format!("highlights.scm: {e}"))?;
        let names: Vec<&str> = NAMES.iter().map(|n| n.0).collect();
        cfg.configure(&names);
        Ok(cfg)
    }
}

fn cfg_language(dir: &Path, symbol: &str) -> Result<tree_sitter::Language, tree_sitter::QueryError> {
    // The library is already loaded (and leaked) by `load`; loading again returns the same handle.
    unsafe {
        let lib = libloading::Library::new(lib_file(dir)).expect("grammar library was loaded before");
        let lib: &'static libloading::Library = Box::leak(Box::new(lib));
        let f: libloading::Symbol<unsafe extern "C" fn() -> *const ()> = lib.get(symbol.as_bytes()).expect("symbol was found before");
        Ok(tree_sitter::Language::new(tree_sitter_language::LanguageFn::from_raw(*f)))
    }
}

thread_local! {
    static LOAD_ERRORS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    static HIGHLIGHTER: std::cell::RefCell<Highlighter> = std::cell::RefCell::new(Highlighter::new());
}

/// Grammar load errors since the last call (shown once as notifications).
pub fn take_load_errors() -> Vec<String> {
    LOAD_ERRORS.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

/// A colored byte range: (start, end, class index into NAMES).
pub type Span = (u32, u32, u8);

/// Highlight the whole text. Ranges are non-overlapping and sorted; unstyled text has no span.
pub fn highlight(g: &Grammar, text: &str) -> Result<Vec<Span>, String> {
    if text.len() > MAX_HIGHLIGHT {
        return Ok(Vec::new());
    }
    let cfg = g.config()?;
    HIGHLIGHTER.with(|h| {
        let mut h = h.borrow_mut();
        let events = h.highlight(cfg, text.as_bytes(), None, None, |_| None).map_err(|e| e.to_string())?;
        let mut out: Vec<Span> = Vec::new();
        let mut stack: Vec<u8> = Vec::new();
        for ev in events {
            match ev.map_err(|e| e.to_string())? {
                HighlightEvent::HighlightStart(Highlight(i)) => stack.push(i as u8),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    if let Some(&c) = stack.last() {
                        match out.last_mut() {
                            Some(last) if last.1 == start as u32 && last.2 == c => last.1 = end as u32,
                            _ => out.push((start as u32, end as u32, c)),
                        }
                    }
                }
            }
        }
        Ok(out)
    })
}

/// Split one line `[ls, le)` into (text, color) runs using `spans` (sorted, may be stale:
/// offsets are clamped and snapped to char boundaries).
pub fn line_runs<'a>(text: &'a str, ls: usize, le: usize, spans: &[Span]) -> Vec<(&'a str, u32)> {
    let mut runs = Vec::new();
    let first = spans.partition_point(|s| (s.1 as usize) <= ls);
    let mut pos = ls;
    for s in &spans[first..] {
        let (mut a, mut b) = ((s.0 as usize).max(ls), (s.1 as usize).min(le));
        if a >= le {
            break;
        }
        while !text.is_char_boundary(a) {
            a += 1;
        }
        while b > a && !text.is_char_boundary(b) {
            b -= 1;
        }
        if a < pos || b <= a {
            continue;
        }
        if a > pos {
            runs.push((&text[pos..a], DEFAULT_FG));
        }
        runs.push((&text[a..b], color_of(s.2)));
        pos = b;
    }
    if pos < le {
        runs.push((&text[pos..le], DEFAULT_FG));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_cover_line_and_clamp() {
        let t = "let x = \"hi\";\nnext";
        let spans = vec![(0, 3, 16), (8, 12, 34), (12, 99, 24)];
        let r = line_runs(t, 0, 13, &spans);
        let joined: String = r.iter().map(|x| x.0).collect();
        assert_eq!(joined, "let x = \"hi\";");
        assert_eq!(r[0], ("let", color_of(16)));
        assert_eq!(r[2], ("\"hi\"", color_of(34)));
        let r2 = line_runs(t, 14, t.len(), &spans);
        assert_eq!(r2.iter().map(|x| x.0).collect::<String>(), "next");
    }
}
