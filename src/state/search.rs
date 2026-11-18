//! Search view (Ctrl+Shift+F) and the title bar CPU/memory meter.

use super::*;
use crate::search::{self, FileHits, Query};
use crate::SearchRow;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(crate) struct SearchState {
    query: Query,
    results: Vec<FileHits>,
    collapsed: Vec<bool>,
    total: usize,
    truncated: bool,
    running: bool,
    generation: u64,
    cancel: Option<search::Cancel>,
    timer: slint::Timer,
    rows: Vec<(usize, usize, usize)>,
    /// remote: matcher used to find columns in returned lines
    remote_matcher: Option<grep_regex::RegexMatcher>,
    error: Option<String>,
}

const NO_HIT: usize = usize::MAX;

impl State {
    pub fn open_search_view(&mut self) {
        // seed with the selected text or the word at the cursor, like VS Code
        if let Some(t) = self.active_tab().filter(|t| t.doc.message.is_empty()) {
            let sel = self.widget_sel();
            let txt = &t.doc.text;
            let seed = if !sel.is_empty() {
                Some(txt[sel.start()..sel.end()].to_string()).filter(|s| !s.contains('\n'))
            } else {
                let (a, b) = ops::word_at(txt, sel.cursor);
                (a < b).then(|| txt[a..b].to_string())
            };
            if let Some(s) = seed {
                self.ui().set_search_text(s.as_str().into());
                self.search_edited(&s);
            }
        }
        self.show_view("search");
        let app = self.ui();
        app.set_search_focus_req(app.get_search_focus_req() + 1);
    }

    pub fn search_edited(&mut self, text: &str) {
        self.search.query.text = text.to_string();
        self.search.timer.start(slint::TimerMode::SingleShot, Duration::from_millis(300), || {
            with_state(|s| s.run_search());
        });
    }

    pub fn search_toggle(&mut self, what: &str) {
        let q = &mut self.search.query;
        match what {
            "case" => q.case = !q.case,
            "word" => q.word = !q.word,
            _ => q.regex = !q.regex,
        }
        let (case, word, regex) = (q.case, q.word, q.regex);
        let app = self.ui();
        app.set_search_case(case);
        app.set_search_word(word);
        app.set_search_regex(regex);
        self.run_search();
    }

    fn cancel_search(&mut self) {
        if let Some(c) = self.search.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.search.generation += 1;
        self.search.running = false;
    }

    pub fn run_search(&mut self) {
        self.cancel_search();
        let s = &mut self.search;
        s.results.clear();
        s.collapsed.clear();
        s.total = 0;
        s.truncated = false;
        s.error = None;
        s.remote_matcher = None;
        let q = s.query.clone();
        let generation = s.generation;
        if q.text.is_empty() {
            self.refresh_search();
            return;
        }
        let Some(root) = self.ws_root() else {
            self.search.error = Some("You have not opened or specified a folder. Only open files are searched.".into());
            self.refresh_search();
            return;
        };
        let m = match search::matcher(&q) {
            Ok(m) => m,
            Err(e) => {
                self.search.error = Some(e);
                self.refresh_search();
                return;
            }
        };
        self.search.running = true;
        match self.ws {
            Ws::Remote(_) => {
                self.search.remote_matcher = Some(m);
                let command = search::remote_command(&root, &q);
                if !self.send(Req::Exec { id: generation, command }) {
                    self.search.running = false;
                }
            }
            _ => {
                let cancel: search::Cancel = Arc::new(AtomicBool::new(false));
                self.search.cancel = Some(cancel.clone());
                std::thread::Builder::new()
                    .name("search".into())
                    .spawn(move || {
                        let res = search::search_local(Path::new(&root), &q, &cancel, |batch| {
                            let _ = slint::invoke_from_event_loop(move || {
                                with_state(|s| s.on_search_batch(generation, batch, 0));
                            });
                        });
                        let _ = slint::invoke_from_event_loop(move || {
                            with_state(|s| s.on_search_done(generation, res.map(|r| r.1)));
                        });
                    })
                    .ok();
            }
        }
        self.refresh_search();
    }

    pub(super) fn on_search_batch(&mut self, generation: u64, batch: Vec<FileHits>, extra: usize) {
        if generation != self.search.generation {
            return;
        }
        let s = &mut self.search;
        for f in batch {
            s.total += f.hits.iter().map(|h| h.ranges.len().max(1)).sum::<usize>();
            match s.results.last_mut() {
                Some(last) if last.rel == f.rel => last.hits.extend(f.hits),
                _ => {
                    s.results.push(f);
                    s.collapsed.push(false);
                }
            }
        }
        let _ = extra;
        self.refresh_search();
    }

    pub(super) fn on_search_done(&mut self, generation: u64, res: Result<bool, String>) {
        if generation != self.search.generation {
            return;
        }
        self.search.running = false;
        self.search.cancel = None;
        match res {
            Ok(truncated) => self.search.truncated = truncated || self.search.total >= search::MAX_RESULTS,
            Err(e) => self.search.error = Some(e),
        }
        self.refresh_search();
    }

    #[cfg(feature = "screenshot")]
    pub(super) fn search_cancel_and_generation(&mut self) -> u64 {
        self.search.timer.stop();
        if let Some(c) = self.search.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.search.generation
    }

    #[cfg(feature = "screenshot")]
    pub(super) fn search_query(&self) -> Query {
        self.search.query.clone()
    }

    pub(super) fn on_exec_lines(&mut self, id: u64, lines: Vec<String>) {
        if id != self.search.generation {
            return;
        }
        let Some(m) = &self.search.remote_matcher else { return };
        let mut batch = Vec::new();
        search::group_remote(m, &lines, &mut batch);
        self.on_search_batch(id, batch, 0);
    }

    pub(super) fn on_exec_done(&mut self, id: u64, error: Option<String>) {
        self.on_search_done(id, error.map_or(Ok(false), Err));
    }

    fn refresh_search(&mut self) {
        let app = self.ui();
        let s = &mut self.search;
        let files = s.results.len();
        let msg = if let Some(e) = &s.error {
            e.clone()
        } else if s.query.text.is_empty() {
            String::new()
        } else if s.total == 0 {
            if s.running { "Searching...".into() } else { "No results found. Review your settings for configured exclusions and check your gitignore files.".into() }
        } else {
            let r = if s.total == 1 { "result" } else { "results" };
            let f = if files == 1 { "file" } else { "files" };
            let more = if s.truncated { " - The result set only contains a subset of all matches. Be more specific in your search to narrow down the results." } else { "" };
            format!("{} {r} in {files} {f}{more}", s.total)
        };
        app.set_search_message(msg.into());
        app.set_search_running(s.running);
        s.rows.clear();
        let mut rows = Vec::new();
        for (fi, f) in s.results.iter().enumerate() {
            let (dir, name) = f.rel.rsplit_once('/').unwrap_or(("", f.rel.as_str()));
            let l = lang::detect(name);
            let count: usize = f.hits.iter().map(|h| h.ranges.len().max(1)).sum();
            rows.push(SearchRow {
                is_file: true,
                expanded: !s.collapsed[fi],
                name: name.into(),
                dir: dir.into(),
                count: count as i32,
                before: SharedString::new(),
                hit: SharedString::new(),
                after: SharedString::new(),
                glyph: l.glyph.into(),
                glyph_color: lang::color(l.color),
            });
            s.rows.push((fi, NO_HIT, 0));
            if s.collapsed[fi] {
                continue;
            }
            for (hi, h) in f.hits.iter().enumerate() {
                let ranges: Vec<(u32, u32)> = if h.ranges.is_empty() { vec![(0, 0)] } else { h.ranges.clone() };
                for (ri, &(a, b)) in ranges.iter().enumerate() {
                    let (a, b) = (a as usize, b as usize);
                    let (before, hit, after) = preview(&h.text, a, b);
                    rows.push(SearchRow {
                        is_file: false,
                        expanded: false,
                        name: SharedString::new(),
                        dir: SharedString::new(),
                        count: 0,
                        before: before.into(),
                        hit: hit.into(),
                        after: after.into(),
                        glyph: SharedString::new(),
                        glyph_color: slint::Color::default(),
                    });
                    s.rows.push((fi, hi, ri));
                }
            }
        }
        self.m.search.set_vec(rows);
    }

    pub fn search_row_clicked(&mut self, row: usize) {
        let Some(&(fi, hi, ri)) = self.search.rows.get(row) else { return };
        if hi == NO_HIT {
            self.search.collapsed[fi] = !self.search.collapsed[fi];
            self.refresh_search();
            return;
        }
        let Some(root) = self.ws_root() else { return };
        let f = &self.search.results[fi];
        let h = &f.hits[hi];
        let (a, b) = h.ranges.get(ri).map(|r| (r.0 as usize, r.1 as usize)).unwrap_or((0, 0));
        let path = self.join(&root, &f.rel.replace('/', &self.sep().to_string()));
        let reveal = (h.line as usize, a, b);
        match self.ws {
            Ws::Remote(_) => {
                let path = self.join(&root, &f.rel);
                self.pending_reveal = Some((Where::Remote(path.clone()), reveal));
                self.open_remote(path.clone());
                // already open: reveal now
                if self.active_tab().is_some_and(|t| t.doc.loc == Where::Remote(path) && t.doc.message.is_empty()) {
                    self.apply_reveal();
                }
            }
            _ => {
                self.open_local(Path::new(&path));
                self.pending_reveal = Some((Where::Local(PathBuf::from(&path)), reveal));
                self.apply_reveal();
            }
        }
    }

    /// Select a search match once its file is loaded.
    pub(super) fn apply_reveal(&mut self) {
        let Some((loc, (line, a, b))) = self.pending_reveal.clone() else { return };
        let Some(i) = self.active else { return };
        if self.tabs[i].doc.loc != loc || !self.tabs[i].doc.message.is_empty() {
            return;
        }
        self.pending_reveal = None;
        let t = &self.tabs[i].doc.text;
        let ls = ops::line_offset(t, line.saturating_sub(1));
        let le = ops::line_end(t, ls);
        let s = ops::floor_char(t, (ls + a).min(le));
        let e = ops::floor_char(t, (ls + b).min(le));
        self.center_line(line.saturating_sub(1));
        self.ui().invoke_focus_editor();
        self.set_sel(Sel::new(s, e));
    }

    pub fn search_action(&mut self, what: &str) {
        match what {
            "refresh" => self.run_search(),
            "clear" => {
                self.ui().set_search_text("".into());
                self.search.query.text.clear();
                self.run_search();
            }
            "collapse" => {
                let all = self.search.collapsed.iter().all(|c| *c);
                for c in &mut self.search.collapsed {
                    *c = !all;
                }
                self.refresh_search();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ CPU / memory meter

    pub fn start_stats(&mut self) {
        self.stats.sample();
        self.stats_timer.start(slint::TimerMode::Repeated, Duration::from_secs(2), || {
            with_state(|s| s.tick_stats());
        });
    }

    fn tick_stats(&mut self) {
        self.stats.sample();
        let app = self.ui();
        app.set_stats_label(self.stats.label().into());
        if app.get_stats_open() {
            self.refresh_stats_graph();
        }
    }

    pub fn refresh_stats_graph(&self) {
        let app = self.ui();
        let cpu_max = self.stats.cpu.iter().copied().fold(5.0f32, f32::max);
        let mem_max = self.stats.mem_mb.iter().copied().fold(1.0f32, f32::max) * 1.2;
        app.set_stats_cpu_path(crate::stats::sparkline(&self.stats.cpu, cpu_max).into());
        app.set_stats_mem_path(crate::stats::sparkline(&self.stats.mem_mb, mem_max).into());
        app.set_stats_cpu_max(format!("{cpu_max:.0}%").into());
        app.set_stats_mem_max(format!("{mem_max:.0} MB").into());
    }
}

/// VS Code-style preview: leading whitespace dropped, long prefixes cut to keep the match visible.
fn preview(text: &str, a: usize, b: usize) -> (String, String, String) {
    let (a, b) = (a.min(text.len()), b.min(text.len()));
    let before = text[..a].trim_start();
    let before = if before.chars().count() > 14 {
        let skip = before.chars().count() - 14;
        format!("…{}", before.chars().skip(skip).collect::<String>())
    } else {
        before.to_string()
    };
    let after: String = text[b..].chars().take(200).collect();
    (before, text[a..b].to_string(), after)
}
