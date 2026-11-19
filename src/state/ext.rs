//! Extensions view, extension commands/keybindings, and extension panels (editor side).

use super::*;
use crate::{ExtBlock, ExtRow};
use rcode_ext::{Action, CommandSpec, Doc as ExtDoc, Kind, Request, Widget};

pub(crate) struct Panel {
    ext: usize,
    id: String,
    title: String,
    langs: Vec<String>,
    /// tab whose document is shown
    source: Option<u64>,
}

impl State {
    pub(super) fn init_extensions(&mut self) {
        self.exts = crate::ext::discover();
        self.settings = crate::ext::Settings::load();
        crate::syntax::set_disabled(self.settings.disabled_grammars.clone());
        let mut errors = Vec::new();
        for x in &mut self.exts {
            if self.settings.enabled_extensions.contains(&x.id) {
                if let Err(e) = x.activate() {
                    x.error = Some(e.clone());
                    errors.push(format!("Extension '{}' could not be activated: {e}", x.name));
                }
            }
        }
        for e in errors {
            self.toast(2, e);
        }
        self.refresh_ext_rows();
    }

    pub(super) fn refresh_ext_rows(&self) {
        let mut rows: Vec<ExtRow> = self
            .exts
            .iter()
            .map(|x| ExtRow {
                kind: 0,
                name: x.name.as_str().into(),
                detail: format!("v{}  ·  {}", x.version, x.publisher).into(),
                description: x.description.as_str().into(),
                enabled: self.settings.enabled_extensions.contains(&x.id),
                error: x.error.clone().unwrap_or_default().into(),
            })
            .collect();
        for g in &crate::syntax::registry().grammars {
            rows.push(ExtRow {
                kind: 1,
                name: g.name.as_str().into(),
                detail: g.extensions.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ").into(),
                description: "Syntax highlighting (tree-sitter)".into(),
                enabled: !self.settings.disabled_grammars.contains(&g.name),
                error: SharedString::new(),
            });
        }
        self.m.ext_rows.set_vec(rows);
        let dir = crate::ext::extensions_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        self.ui().set_ext_dir(dir.into());
    }

    pub fn ext_toggle(&mut self, idx: usize) {
        if let Some(x) = self.exts.get_mut(idx) {
            let id = x.id.clone();
            if self.settings.enabled_extensions.remove(&id) {
                x.deactivate();
                if self.panel.as_ref().is_some_and(|p| p.ext == idx) {
                    self.panel_close();
                }
            } else {
                match x.activate() {
                    Ok(()) => {
                        x.error = None;
                        self.settings.enabled_extensions.insert(id);
                    }
                    Err(e) => {
                        x.error = Some(e.clone());
                        let name = x.name.clone();
                        self.toast(2, format!("Extension '{name}' could not be activated: {e}"));
                    }
                }
            }
        } else if let Some(g) = crate::syntax::registry().grammars.get(idx - self.exts.len()) {
            if !self.settings.disabled_grammars.remove(&g.name) {
                self.settings.disabled_grammars.insert(g.name.clone());
            }
            crate::syntax::set_disabled(self.settings.disabled_grammars.clone());
            for t in &mut self.tabs {
                if t.doc.message.is_empty() {
                    t.doc.detect_grammar();
                }
            }
            self.refresh_visible();
            self.schedule_highlight();
            self.refresh_status();
        }
        if let Err(e) = self.settings.save() {
            self.toast(1, format!("Could not save settings: {e}"));
        }
        self.refresh_ext_rows();
        self.refresh_title_button();
    }

    /// Commands of all active extensions.
    pub(super) fn ext_commands(&self) -> Vec<(usize, CommandSpec)> {
        self.exts
            .iter()
            .enumerate()
            .filter(|(_, x)| x.is_loaded() && self.settings.enabled_extensions.contains(&x.id))
            .flat_map(|(i, x)| x.contributes.commands.iter().map(move |c| (i, c.clone())))
            .collect()
    }

    fn active_ext_doc(&self) -> Option<ExtDoc> {
        let t = self.active_tab().filter(|t| t.doc.message.is_empty())?;
        Some(ExtDoc { path: t.doc.loc.path_string(), lang: t.doc.lang_name().to_string(), text: t.doc.text.clone() })
    }

    fn doc_of(&self, tab: u64) -> Option<ExtDoc> {
        let t = &self.tabs[self.tab_index(tab)?];
        Some(ExtDoc { path: t.doc.loc.path_string(), lang: t.doc.lang_name().to_string(), text: t.doc.text.clone() })
    }

    pub(super) fn run_ext_command(&mut self, ext: usize, id: &str) {
        let doc = self.active_ext_doc();
        let Some(x) = self.exts.get(ext) else { return };
        match x.call(&Request::Command { id: id.to_string(), doc }) {
            Ok(r) => self.ext_actions(ext, r.actions),
            Err(e) => {
                self.toast(2, format!("{}: {e}", x.name));
            }
        }
    }

    fn ext_actions(&mut self, ext: usize, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::OpenPanel { panel } => self.panel_open(ext, &panel),
                Action::ClosePanel { panel } => {
                    if self.panel.as_ref().is_some_and(|p| p.ext == ext && p.id == panel) {
                        self.panel_close();
                    }
                }
                Action::OpenUrl { url } => open_url(&url),
                Action::OpenFile { path } => self.open_path(&path),
                Action::Notify { message, level } => {
                    self.toast(level.min(2) as i32, message);
                }
            }
        }
    }

    /// Keybindings contributed by extensions. `chord`: this is the key after Ctrl+K.
    pub(super) fn ext_key(&mut self, chord: bool, k: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        let lang = self.active_tab().map(|t| t.doc.lang_name().to_string()).unwrap_or_default();
        let hit = self.ext_commands().into_iter().find(|(_, c)| {
            if !c.languages.is_empty() && !c.languages.contains(&lang) {
                return false;
            }
            let parts: Vec<&str> = c.key.split_whitespace().collect();
            match (chord, parts.as_slice()) {
                (false, [one]) => crate::ext::key_part_matches(one, k, ctrl, shift, alt),
                (true, [first, second]) => first.eq_ignore_ascii_case("ctrl+k") && crate::ext::key_part_matches(second, k, ctrl, shift, alt),
                _ => false,
            }
        });
        match hit {
            Some((ei, c)) => {
                self.run_ext_command(ei, &c.id);
                true
            }
            None => false,
        }
    }

    pub(super) fn refresh_title_button(&self) {
        let lang = self.active_tab().filter(|t| t.doc.message.is_empty()).map(|t| t.doc.lang_name().to_string());
        let show = lang.is_some_and(|l| self.ext_commands().iter().any(|(_, c)| c.editor_title_button && c.languages.contains(&l)));
        self.ui().set_title_button_visible(show);
    }

    pub fn title_button_clicked(&mut self) {
        let lang = self.active_tab().map(|t| t.doc.lang_name().to_string()).unwrap_or_default();
        if let Some((ei, c)) = self.ext_commands().into_iter().find(|(_, c)| c.editor_title_button && c.languages.contains(&lang)) {
            self.run_ext_command(ei, &c.id);
        }
    }

    // ------------------------------------------------------------------ panel

    fn panel_open(&mut self, ext: usize, id: &str) {
        let Some(spec) = self.exts.get(ext).and_then(|x| x.contributes.panels.iter().find(|p| p.id == id)).cloned() else { return };
        let source = self
            .active_tab()
            .filter(|t| spec.languages.is_empty() || spec.languages.iter().any(|l| l == t.doc.lang_name()))
            .map(|t| t.id);
        self.panel = Some(Panel { ext, id: spec.id, title: spec.title, langs: spec.languages, source });
        self.ui().set_panel_open(true);
        self.panel_render();
        self.ui().invoke_focus_editor();
    }

    pub fn panel_close(&mut self) {
        self.panel = None;
        self.panel_timer.stop();
        let app = self.ui();
        app.set_panel_open(false);
        self.m.panel_blocks.set_vec(Vec::new());
    }

    /// Called after a tab switch or an edit: follow the active document and re-render soon.
    pub(super) fn panel_follow(&mut self) {
        self.refresh_title_button();
        let Some(p) = &self.panel else { return };
        let Some(t) = self.active_tab() else { return };
        let matches = p.langs.is_empty() || p.langs.iter().any(|l| l == t.doc.lang_name());
        if !matches || !t.doc.message.is_empty() {
            return; // keep showing the last document, like VS Code
        }
        let id = t.id;
        if let Some(p) = &mut self.panel {
            p.source = Some(id);
        }
        self.panel_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(120), || {
            with_state(|s| s.panel_render());
        });
    }

    #[cfg(feature = "screenshot")]
    pub(super) fn panel_render_now(&mut self) {
        self.panel_timer.stop();
        self.panel_render();
    }

    fn panel_render(&mut self) {
        let Some(p) = &self.panel else { return };
        let (ext, id, title) = (p.ext, p.id.clone(), p.title.clone());
        let Some(doc) = p.source.and_then(|t| self.doc_of(t)) else {
            self.ui().set_panel_title(title.into());
            self.m.panel_blocks.set_vec(Vec::new());
            return;
        };
        let name = doc.path.rsplit(['/', '\\']).next().unwrap_or("").to_string();
        self.ui().set_panel_title(format!("{title} {name}").into());
        let Some(x) = self.exts.get(ext) else { return };
        match x.call(&Request::Render { panel: id, doc }) {
            Ok(r) => {
                let blocks: Vec<ExtBlock> = r.widgets.unwrap_or_default().iter().map(to_block).collect();
                self.m.panel_blocks.set_vec(blocks);
                self.ext_actions(ext, r.actions);
            }
            Err(e) => {
                let n = x.name.clone();
                self.toast(2, format!("{n}: {e}"));
            }
        }
    }

    pub fn panel_link(&mut self, url: &str) {
        let url = url.trim();
        if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
            open_url(url);
            return;
        }
        if url.starts_with('#') || url.is_empty() {
            return;
        }
        let Some(src) = self.panel.as_ref().and_then(|p| p.source).and_then(|t| self.doc_of(t)) else { return };
        let rel = url.split('#').next().unwrap_or("").replace("%20", " ");
        if rel.is_empty() {
            return;
        }
        let dir = self.parent_of(&src.path);
        let path = if rel.starts_with('/') { rel } else { self.join(&dir, &rel) };
        self.open_path(&path);
    }

    pub fn panel_button(&mut self, target: &str) {
        let Some(p) = &self.panel else { return };
        let (ext, id) = (p.ext, p.id.clone());
        let doc = p.source.and_then(|t| self.doc_of(t));
        let Some(x) = self.exts.get(ext) else { return };
        match x.call(&Request::Click { panel: id, target: target.to_string(), doc }) {
            Ok(r) => self.ext_actions(ext, r.actions),
            Err(e) => {
                let n = x.name.clone();
                self.toast(2, format!("{n}: {e}"));
            }
        }
    }
}

fn styled(md: &str) -> slint::StyledText {
    slint::StyledText::from_markdown(md).unwrap_or_else(|_| slint::StyledText::from_plain_text(md))
}

fn kind_id(k: Kind) -> i32 {
    match k {
        Kind::Heading => 0,
        Kind::Paragraph => 1,
        Kind::ListItem => 2,
        Kind::Code => 3,
        Kind::Rule => 4,
        Kind::TableRow => 5,
        Kind::Button => 6,
        Kind::Label => 7,
    }
}

fn code_lines(lang: &str, text: &str) -> Vec<HlLine> {
    let spans = crate::syntax::registry().by_hint(lang).and_then(|g| crate::syntax::highlight(g, text).ok()).unwrap_or_default();
    let mut out = Vec::new();
    let mut ls = 0;
    for (i, line) in text.split('\n').enumerate() {
        let le = ls + line.len();
        let runs: Vec<HlSpan> = crate::syntax::line_runs(text, ls, le, &spans)
            .into_iter()
            .map(|(t, c)| HlSpan { text: t.replace('\t', "    ").into(), color: lang::color(c) })
            .collect();
        out.push(HlLine { line: i as i32, spans: ModelRc::from(Rc::new(VecModel::from(runs))) });
        ls = le + 1;
    }
    out
}

fn to_block(w: &Widget) -> ExtBlock {
    let text = match w.kind {
        // headings are drawn bold; the inline renderer has no heading style
        Kind::Heading => styled(&format!("**{}**", w.text)),
        Kind::Paragraph | Kind::ListItem => styled(&w.text),
        _ => slint::StyledText::default(),
    };
    let cells: Vec<slint::StyledText> = w.cells.iter().map(|c| if w.header { styled(&format!("**{c}**")) } else { styled(c) }).collect();
    ExtBlock {
        kind: kind_id(w.kind),
        text,
        plain: if matches!(w.kind, Kind::Label | Kind::Button) { w.text.as_str().into() } else { SharedString::new() },
        level: w.level as i32,
        marker: w.marker.as_str().into(),
        cells: ModelRc::from(Rc::new(VecModel::from(cells))),
        header: w.header,
        id: w.id.as_str().into(),
        quote: w.quote as i32,
        indent: w.indent as i32,
        code: if w.kind == Kind::Code { ModelRc::from(Rc::new(VecModel::from(code_lines(&w.lang, &w.text)))) } else { ModelRc::default() },
    }
}

/// Open an http(s)/mailto link in the default browser.
pub(super) fn open_url(url: &str) {
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")) {
        return;
    }
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    let _ = r;
}
