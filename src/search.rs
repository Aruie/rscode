//! Search across files (Ctrl+Shift+F), like VS Code: nothing is indexed ahead of time, every
//! search walks the folder with ripgrep's own libraries, skipping .gitignore'd and binary files,
//! and streams results as they are found. Remote folders run `rg` (or `grep`) on the host.

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{sinks, BinaryDetection, SearcherBuilder};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MAX_RESULTS: usize = 20_000;
/// Long lines (minified files) are cut for the preview.
const MAX_LINE: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Query {
    pub text: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
}

#[derive(Clone, Debug)]
pub struct Hit {
    /// 1-based line number.
    pub line: u32,
    pub text: String,
    /// Byte ranges of the matches inside `text`.
    pub ranges: Vec<(u32, u32)>,
}

#[derive(Clone, Debug)]
pub struct FileHits {
    /// Path relative to the searched folder, with '/' separators.
    pub rel: String,
    pub hits: Vec<Hit>,
}

pub fn matcher(q: &Query) -> Result<RegexMatcher, String> {
    RegexMatcherBuilder::new()
        .case_insensitive(!q.case)
        .word(q.word)
        .fixed_strings(!q.regex)
        .build(&q.text)
        .map_err(|e| e.to_string())
}

fn trim_line(line: &str) -> &str {
    let line = line.trim_end_matches(['\n', '\r']);
    if line.len() <= MAX_LINE {
        return line;
    }
    let mut e = MAX_LINE;
    while !line.is_char_boundary(e) {
        e -= 1;
    }
    &line[..e]
}

/// Build a hit for one line; None if the matcher finds nothing in it.
pub fn hit_in_line(m: &RegexMatcher, lnum: u32, line: &str) -> Option<Hit> {
    let text = trim_line(line);
    let mut ranges = Vec::new();
    let _ = m.find_iter(text.as_bytes(), |r| {
        if r.end() > r.start() {
            ranges.push((r.start() as u32, r.end() as u32));
        }
        ranges.len() < 200
    });
    if ranges.is_empty() {
        return None;
    }
    Some(Hit { line: lnum, text: text.to_string(), ranges })
}

/// Search a local folder. `emit` receives batches; returns (results, truncated).
pub fn search_local(root: &Path, q: &Query, cancel: &AtomicBool, mut emit: impl FnMut(Vec<FileHits>)) -> Result<(usize, bool), String> {
    let m = matcher(q)?;
    let mut searcher = SearcherBuilder::new().line_number(true).binary_detection(BinaryDetection::quit(0)).build();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(e.file_type().is_some_and(|t| t.is_dir()) && crate::remote::SKIP_DIRS.contains(&n.as_ref()))
        })
        .max_filesize(Some(20 * 1024 * 1024))
        .build();
    let mut total = 0;
    let mut batch = Vec::new();
    let mut last = Instant::now();
    for entry in walker.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return Ok((total, false));
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let mut hits = Vec::new();
        let _ = searcher.search_path(
            &m,
            path,
            sinks::UTF8(|lnum, line| {
                if let Some(h) = hit_in_line(&m, lnum as u32, line) {
                    total += h.ranges.len();
                    hits.push(h);
                }
                Ok(total < MAX_RESULTS)
            }),
        );
        if !hits.is_empty() {
            let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
            batch.push(FileHits { rel, hits });
        }
        if !batch.is_empty() && last.elapsed() > Duration::from_millis(80) {
            emit(std::mem::take(&mut batch));
            last = Instant::now();
        }
        if total >= MAX_RESULTS {
            emit(batch);
            return Ok((total, true));
        }
    }
    emit(batch);
    Ok((total, false))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Shell command that prints `./path:line:text` lines on the remote host.
pub fn remote_command(root: &str, q: &Query) -> String {
    let pat = shell_quote(&q.text);
    let mut rg = String::from("rg -n --no-heading --color never --with-filename --max-columns 1000 --max-filesize 20M");
    let mut grep = String::from("grep -rnIH");
    if !q.case {
        rg.push_str(" -i");
        grep.push_str(" -i");
    }
    if q.word {
        rg.push_str(" -w");
        grep.push_str(" -w");
    }
    if !q.regex {
        rg.push_str(" -F");
        grep.push_str(" -F");
    } else {
        grep.push_str(" -E");
    }
    for d in crate::remote::SKIP_DIRS {
        grep.push_str(&format!(" --exclude-dir={d}"));
    }
    format!(
        "cd {} && {{ if command -v rg >/dev/null 2>&1; then {rg} -e {pat} .; else {grep} -e {pat} .; fi; }} 2>/dev/null | head -n {}",
        shell_quote(root),
        MAX_RESULTS
    )
}

/// Parse one `./path:line:text` output line.
pub fn parse_remote_line(line: &str) -> Option<(&str, u32, &str)> {
    let b = line.as_bytes();
    let mut i = 0;
    while let Some(off) = line[i..].find(':') {
        let c = i + off;
        let digits = b[c + 1..].iter().take_while(|x| x.is_ascii_digit()).count();
        if digits > 0 && b.get(c + 1 + digits) == Some(&b':') {
            let path = line[..c].trim_start_matches("./");
            let lnum = line[c + 1..c + 1 + digits].parse().ok()?;
            return Some((path, lnum, &line[c + 2 + digits..]));
        }
        i = c + 1;
    }
    None
}

/// Group parsed remote lines into per-file hits, appending to `out`.
pub fn group_remote(m: &RegexMatcher, lines: &[String], out: &mut Vec<FileHits>) -> usize {
    let mut n = 0;
    for l in lines {
        let Some((path, lnum, text)) = parse_remote_line(l) else { continue };
        // matches are found again locally to get their columns
        let hit = hit_in_line(m, lnum, text).unwrap_or(Hit { line: lnum, text: trim_line(text).to_string(), ranges: Vec::new() });
        n += hit.ranges.len().max(1);
        match out.last_mut() {
            Some(f) if f.rel == path => f.hits.push(hit),
            _ => out.push(FileHits { rel: path.to_string(), hits: vec![hit] }),
        }
    }
    n
}

pub type Cancel = Arc<AtomicBool>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_ranges() {
        let q = Query { text: "foo".into(), ..Default::default() };
        let m = matcher(&q).unwrap();
        let h = hit_in_line(&m, 3, "Foo foo food\n").unwrap();
        assert_eq!(h.ranges.len(), 3);
        let m = matcher(&Query { word: true, case: true, ..q.clone() }).unwrap();
        assert_eq!(hit_in_line(&m, 1, "Foo foo food").unwrap().ranges, vec![(4, 7)]);
        let m = matcher(&Query { text: "a.c".into(), ..Default::default() }).unwrap();
        assert!(hit_in_line(&m, 1, "abc").is_none());
    }

    #[test]
    fn parses_remote_output() {
        assert_eq!(parse_remote_line("./src/a:b.rs:12:let x = 1;"), Some(("src/a:b.rs", 12, "let x = 1;")));
        assert_eq!(parse_remote_line("./x.txt:3:a:1:b"), Some(("x.txt", 3, "a:1:b")));
        assert!(parse_remote_line("garbage").is_none());
        let cmd = remote_command("/home/it's", &Query { text: "a'b".into(), ..Default::default() });
        assert!(cmd.contains(r"'/home/it'\''s'") && cmd.contains(r"-e 'a'\''b'"));
    }

    #[test]
    fn local_search_respects_folder() {
        let root = std::env::temp_dir().join(format!("rcode-search-{}", std::process::id()));
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(root.join("a.txt"), "needle here\nno\nneedle again needle\n").unwrap();
        std::fs::write(root.join("node_modules/b.txt"), "needle\n").unwrap();
        let mut files = Vec::new();
        let q = Query { text: "needle".into(), ..Default::default() };
        let (n, trunc) = search_local(&root, &q, &AtomicBool::new(false), |b| files.extend(b)).unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert_eq!((n, trunc), (3, false));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hits.iter().map(|h| h.line).collect::<Vec<_>>(), vec![1, 3]);
    }
}
