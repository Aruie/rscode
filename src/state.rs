//! Application state and all UI behavior. Lives on the UI thread in a thread_local.

use crate::commands::{self, Entry, COMMANDS};
use crate::doc::{Doc, Where};
use crate::lang;
use crate::remote::{self, Event, Req};
use crate::ssh_config::{self, HostEntry};
use crate::text_ops::{self as ops, Edit, Sel};
use crate::tree::Tree;
use crate::{App, Crumb, ExtBlock, ExtRow, GroupData, HlLine, HlSpan, HostRow, MenuItemData, QuickItem, SearchRow, TabItem, Toast, TreeRow};
use slint::platform::Key;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

thread_local! {
    pub static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Run `f` on the state. Returns None if the state is already borrowed (re-entrant callback).
pub fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

/// Run something on the UI thread after the current callback returned.
fn later(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, f);
}

fn is_key(text: &str, k: Key) -> bool {
    text == SharedString::from(k).as_str()
}

/// With the Korean IME in Hangul mode, Ctrl/Alt shortcuts arrive as jamo ("ㄴ" for S).
/// VS Code matches the physical key, so map 2-beolsik jamo back to the Latin letter on that key.
pub(crate) fn shortcut_key(text: &str, ctrl: bool, alt: bool) -> std::borrow::Cow<'_, str> {
    if !(ctrl || alt) {
        return text.into();
    }
    let mut it = text.chars();
    let (Some(c), None) = (it.next(), it.next()) else { return text.into() };
    let latin = match c {
        'ㅂ' | 'ㅃ' => 'q',
        'ㅈ' | 'ㅉ' => 'w',
        'ㄷ' | 'ㄸ' => 'e',
        'ㄱ' | 'ㄲ' => 'r',
        'ㅅ' | 'ㅆ' => 't',
        'ㅛ' => 'y',
        'ㅕ' => 'u',
        'ㅑ' => 'i',
        'ㅐ' | 'ㅒ' => 'o',
        'ㅔ' | 'ㅖ' => 'p',
        'ㅁ' => 'a',
        'ㄴ' => 's',
        'ㅇ' => 'd',
        'ㄹ' => 'f',
        'ㅎ' => 'g',
        'ㅗ' => 'h',
        'ㅓ' => 'j',
        'ㅏ' => 'k',
        'ㅣ' => 'l',
        'ㅋ' => 'z',
        'ㅌ' => 'x',
        'ㅊ' => 'c',
        'ㅍ' => 'v',
        'ㅠ' => 'b',
        'ㅜ' => 'n',
        'ㅡ' => 'm',
        _ => return text.into(),
    };
    latin.to_string().into()
}

const MAX_FILE: u64 = 20 * 1024 * 1024;
const BINARY_MSG: &str = "The file is not displayed in the text editor because it is either binary or uses an unsupported text encoding.";
const LARGE_MSG: &str = "The file is not displayed in the text editor because it is too large (over 20 MB).";

pub struct Tab {
    id: u64,
    doc: Doc,
    scroll: (f32, f32),
    close_after_save: bool,
    /// editor group the tab lives in (0 left, 1 right)
    group: usize,
    /// set for extension preview tabs
    preview: Option<ext::PreviewOf>,
}

/// Per editor group view state.
struct Group {
    active: Option<u64>,
    view_first: usize,
    view_rows: usize,
    line_count: usize,
    cursor_line: usize,
    hl_active: bool,
    hl: Rc<VecModel<HlLine>>,
    tabs_m: Rc<VecModel<TabItem>>,
    crumbs_m: Rc<VecModel<Crumb>>,
    blocks_m: Rc<VecModel<ExtBlock>>,
}

impl Group {
    fn new() -> Self {
        Group {
            active: None,
            view_first: 0,
            view_rows: 60,
            line_count: 1,
            cursor_line: 0,
            hl_active: false,
            hl: Rc::new(VecModel::default()),
            tabs_m: Rc::new(VecModel::default()),
            crumbs_m: Rc::new(VecModel::default()),
            blocks_m: Rc::new(VecModel::default()),
        }
    }
}

#[derive(Clone, PartialEq)]
enum Ws {
    None,
    Local(PathBuf),
    Remote(String),
}

#[derive(Default)]
struct RemoteConn {
    tx: Option<UnboundedSender<Req>>,
    generation: u64,
    host: Option<HostEntry>,
    connected: bool,
    home: String,
    toast: Option<u64>,
}

#[derive(Clone, Copy, PartialEq)]
enum PickKind {
    RemoteMenu,
    Hosts,
    HostKey,
    Eol,
    Lang,
    Indent,
}

#[derive(Clone, PartialEq)]
enum InputKind {
    NewFile(String),
    NewFolder(String),
    Rename(String, bool),
}

#[derive(Clone, Copy, PartialEq)]
enum BrowsePurpose {
    OpenFolder,
    OpenFile,
    SaveAs(u64),
}

enum QMode {
    Nav,
    Pick(PickKind),
    Input(InputKind),
    Secret(Option<oneshot::Sender<Option<String>>>),
    Browse(BrowsePurpose),
}

#[derive(Clone)]
enum QAction {
    None,
    ExtCmd(usize, String),
    Open(String),
    Tab(u64),
    Cmd(&'static str),
    Host(usize),
    AdHoc(String),
    Choice(usize),
    BrowseOk,
    BrowseInto(String, bool),
}

#[derive(Clone)]
struct QRow {
    label: String,
    desc: String,
    glyph: &'static str,
    color: u32,
    key: std::borrow::Cow<'static, str>,
    action: QAction,
}

impl QRow {
    fn text(label: impl Into<String>, action: QAction) -> Self {
        QRow { label: label.into(), desc: String::new(), glyph: "", color: 0, key: "".into(), action }
    }
}

#[derive(Clone)]
enum Then {
    Quit,
    CloseFolder,
    CloseEditors,
    OpenLocal(PathBuf),
    Connect(HostEntry),
    Disconnect,
}

enum Dialog {
    CloseTab(u64),
    DiscardAll(Then),
    Delete(String, bool),
}

enum Popup {
    Menu(Vec<&'static str>),
    Context(Vec<&'static str>),
}

#[derive(Default)]
struct Find {
    open: bool,
    replace: bool,
    text: String,
    replace_text: String,
    case: bool,
    word: bool,
}

struct Models {
    tree: Rc<VecModel<TreeRow>>,
    quick: Rc<VecModel<QuickItem>>,
    toasts: Rc<VecModel<Toast>>,
    hosts: Rc<VecModel<HostRow>>,
    popup: Rc<VecModel<MenuItemData>>,
    search: Rc<VecModel<SearchRow>>,
    ext_rows: Rc<VecModel<ExtRow>>,
}

pub struct State {
    app: Weak<App>,
    m: Models,
    tabs: Vec<Tab>,
    /// index in `tabs` of the active tab of the current group (kept in sync by `sync_active`)
    active: Option<usize>,
    groups: [Group; 2],
    /// current (focused) editor group
    cur: usize,
    split: bool,
    mru: Vec<u64>,
    closed: Vec<Where>,
    next_id: u64,
    untitled: usize,
    ws: Ws,
    tree: Tree,
    remote: RemoteConn,
    hosts: Vec<HostEntry>,
    q_open: bool,
    q_mode: QMode,
    q_rows: Vec<QRow>,
    q_all: Vec<QRow>,
    q_browse_dir: String,
    q_browse_entries: Vec<(String, bool)>,
    q_return_tree: bool,
    hostkey_reply: Option<oneshot::Sender<bool>>,
    file_index: Option<Vec<String>>,
    index_loading: bool,
    chord: bool,
    find: Find,
    toasts: Vec<(u64, Toast)>,
    toast_seq: u64,
    dialog: Option<Dialog>,
    popup: Option<Popup>,
    font_size: f32,
    sidebar: bool,
    view: &'static str,
    pending_then: Option<Then>,
    pending_saves: usize,
    save_failed: bool,
    hl_timer: slint::Timer,
    search: search::SearchState,
    /// (file, (1-based line, start col, end col)) to select once the file is shown
    pending_reveal: Option<(Where, (usize, usize, usize))>,
    stats: crate::stats::Stats,
    stats_timer: slint::Timer,
    exts: Vec<crate::ext::Ext>,
    settings: crate::ext::Settings,
    preview_timer: slint::Timer,
}

fn rows_model<T: Clone + 'static>(m: &Rc<VecModel<T>>) -> ModelRc<T> {
    ModelRc::from(m.clone())
}

impl State {
    pub fn new(app: &App) -> Self {
        let m = Models {
            tree: Rc::new(VecModel::default()),
            quick: Rc::new(VecModel::default()),
            toasts: Rc::new(VecModel::default()),
            hosts: Rc::new(VecModel::default()),
            popup: Rc::new(VecModel::default()),
            search: Rc::new(VecModel::default()),
            ext_rows: Rc::new(VecModel::default()),
        };
        app.set_tree_rows(rows_model(&m.tree));
        app.set_quick_items(rows_model(&m.quick));
        app.set_toasts(rows_model(&m.toasts));
        app.set_hosts(rows_model(&m.hosts));
        app.set_popup_items(rows_model(&m.popup));
        app.set_search_rows(rows_model(&m.search));
        app.set_ext_rows(rows_model(&m.ext_rows));
        let mut s = State {
            app: app.as_weak(),
            m,
            tabs: Vec::new(),
            active: None,
            groups: [Group::new(), Group::new()],
            cur: 0,
            split: false,
            mru: Vec::new(),
            closed: Vec::new(),
            next_id: 1,
            untitled: 0,
            ws: Ws::None,
            tree: Tree::default(),
            remote: RemoteConn::default(),
            hosts: Vec::new(),
            q_open: false,
            q_mode: QMode::Nav,
            q_rows: Vec::new(),
            q_all: Vec::new(),
            q_browse_dir: String::new(),
            q_browse_entries: Vec::new(),
            q_return_tree: false,
            hostkey_reply: None,
            file_index: None,
            index_loading: false,
            chord: false,
            find: Find::default(),
            toasts: Vec::new(),
            toast_seq: 0,
            dialog: None,
            popup: None,
            font_size: 14.0,
            sidebar: true,
            view: "explorer",
            pending_then: None,
            pending_saves: 0,
            save_failed: false,
            hl_timer: slint::Timer::default(),
            search: Default::default(),
            pending_reveal: None,
            stats: crate::stats::Stats::new(),
            stats_timer: slint::Timer::default(),
            exts: Vec::new(),
            settings: Default::default(),
            preview_timer: slint::Timer::default(),
        };
        s.reload_hosts();
        s.init_extensions();
        s.start_stats();
        s.refresh_title();
        s.refresh_status();
        s.push_group(0);
        s.push_group(1);
        s
    }

    fn ui(&self) -> App {
        self.app.upgrade().expect("app alive")
    }

    // ------------------------------------------------------------------ helpers

    fn sep(&self) -> char {
        if matches!(self.ws, Ws::Remote(_)) { '/' } else { std::path::MAIN_SEPARATOR }
    }

    fn join(&self, dir: &str, name: &str) -> String {
        match self.ws {
            Ws::Remote(_) => remote::join(dir, name),
            _ => Path::new(dir).join(name).to_string_lossy().into_owned(),
        }
    }

    fn parent_of(&self, path: &str) -> String {
        match self.ws {
            Ws::Remote(_) => remote::parent(path),
            _ => Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        }
    }

    fn ws_root(&self) -> Option<String> {
        match &self.ws {
            Ws::None => None,
            Ws::Local(p) => Some(p.to_string_lossy().into_owned()),
            Ws::Remote(p) => Some(p.clone()),
        }
    }

    fn ws_name(&self) -> String {
        match &self.ws {
            Ws::None => String::new(),
            Ws::Local(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned()),
            Ws::Remote(p) => {
                let n = p.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                if n.is_empty() { "/".into() } else { n.into() }
            }
        }
    }

    fn rel_path(&self, full: &str) -> String {
        if let Some(root) = self.ws_root() {
            if let Some(r) = full.strip_prefix(&root) {
                return r.trim_start_matches(['/', '\\']).to_string();
            }
        }
        full.to_string()
    }

    fn remote_label(&self) -> String {
        match &self.remote.host {
            Some(h) if self.remote.connected => format!("SSH: {}", h.alias),
            _ => String::new(),
        }
    }

    fn send(&mut self, req: Req) -> bool {
        match &self.remote.tx {
            Some(tx) if self.remote.connected && tx.send(req).is_ok() => true,
            _ => {
                self.toast(2, "Not connected to the remote host.".into());
                false
            }
        }
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.active.map(|i| &self.tabs[i])
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn g(&self) -> i32 {
        self.cur as i32
    }

    fn widget_sel(&self) -> Sel {
        let app = self.ui();
        Sel::new(app.invoke_editor_anchor(self.g()).max(0) as usize, app.invoke_editor_cursor(self.g()).max(0) as usize)
    }

    fn focus_ed(&self) {
        self.ui().invoke_focus_editor(self.g());
    }

    fn scroll_y(&self) -> f32 {
        self.ui().invoke_editor_scroll_y(self.g())
    }

    fn set_scroll_y(&self, y: f32) {
        let app = self.ui();
        app.invoke_set_scroll(self.g(), app.invoke_editor_scroll_x(self.g()), y);
    }

    /// Recompute `active` from the current group.
    fn sync_active(&mut self) {
        self.active = self.groups[self.cur].active.and_then(|id| self.tab_index(id));
    }

    /// Tabs of group `g`, in display order (indices into `tabs`).
    fn group_tabs(&self, g: usize) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].group == g).collect()
    }

    fn active_in(&self, g: usize) -> Option<usize> {
        self.groups[g].active.and_then(|id| self.tab_index(id))
    }

    /// Send group `g`'s view data to the UI.
    fn push_group(&self, g: usize) {
        let app = self.ui();
        let gr = &self.groups[g];
        let t = self.active_in(g).map(|i| &self.tabs[i]);
        let kind = match t {
            None => 0,
            Some(t) if t.preview.is_some() => 3,
            Some(t) if !t.doc.message.is_empty() => 2,
            Some(_) => 1,
        };
        let n = gr.line_count.max(1);
        let data = GroupData {
            tabs: rows_model(&gr.tabs_m),
            crumbs: rows_model(&gr.crumbs_m),
            kind,
            message: t.map(|t| t.doc.message.as_str()).unwrap_or("").into(),
            read_only: t.is_some_and(|t| t.doc.read_only),
            line_count: n as i32,
            line_digits: n.to_string().len() as i32,
            cursor_line: gr.cursor_line as i32,
            hl_active: gr.hl_active && kind == 1,
            hl_lines: rows_model(&gr.hl),
            blocks: rows_model(&gr.blocks_m),
            title_button: kind == 1 && self.title_button_for(g),
            active: g == self.cur && (self.split || g == 0),
        };
        if g == 0 {
            app.set_group0(data);
        } else {
            app.set_group1(data);
        }
        app.set_split(self.split);
    }

    pub fn is_cur(&self, g: usize) -> bool {
        g == self.cur
    }

    /// Make group `g` the current one (keyboard focus moved there).
    pub fn use_group(&mut self, g: usize) {
        if g == self.cur || (g == 1 && !self.split) {
            return;
        }
        self.save_view_state();
        self.cur = g;
        self.sync_active();
        self.refresh_tabs();
        self.refresh_status();
        self.refresh_title();
        if self.find.open {
            self.find_count();
        }
    }

    // ------------------------------------------------------------------ toasts

    pub fn toast(&mut self, kind: i32, message: String) -> u64 {
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.toasts.push((id, Toast { message: message.into(), kind }));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        self.sync_toasts();
        if kind == 0 || kind == 1 {
            slint::Timer::single_shot(Duration::from_secs(8), move || {
                with_state(|s| s.remove_toast(id));
            });
        }
        id
    }

    fn remove_toast(&mut self, id: u64) {
        self.toasts.retain(|(i, _)| *i != id);
        self.sync_toasts();
    }

    fn sync_toasts(&self) {
        self.m.toasts.set_vec(self.toasts.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>());
    }

    pub fn toast_close(&mut self, idx: usize) {
        if let Some((id, _)) = self.toasts.get(idx) {
            let id = *id;
            self.remove_toast(id);
        }
    }

    // ------------------------------------------------------------------ title / status

    fn refresh_title(&self) {
        let app = self.ui();
        let folder = self.ws_name();
        let remote = self.remote_label();
        let remote_sfx = if remote.is_empty() { String::new() } else { format!(" [{remote}]") };
        let mut parts = Vec::new();
        if let Some(t) = self.active_tab() {
            let dirty = if t.doc.is_dirty() { "● " } else { "" };
            parts.push(format!("{dirty}{}", t.doc.loc.file_name()));
        }
        if !folder.is_empty() {
            parts.push(format!("{folder}{remote_sfx}"));
        }
        parts.push("rcode".into());
        app.set_window_title(parts.join(" - ").into());
        let cc = if folder.is_empty() {
            if remote.is_empty() { "rcode".to_string() } else { format!("[{remote}]") }
        } else {
            format!("{folder}{remote_sfx}")
        };
        app.set_command_center_text(cc.into());
        app.set_workspace_name(folder.to_uppercase().into());
        app.set_status_remote(remote.into());
        app.set_status_remote_busy(self.remote.tx.is_some() && !self.remote.connected);
    }

    fn refresh_status(&mut self) {
        let app = self.ui();
        let Some(t) = self.active_tab().filter(|t| t.preview.is_none() && t.doc.message.is_empty()) else {
            app.set_editor_visible(false);
            return;
        };
        app.set_editor_visible(true);
        let d = &t.doc;
        let sel = d.sel;
        let (l, c) = ops::line_col(&d.text, sel.cursor.min(d.text.len()), d.tab_size);
        let mut lncol = format!("Ln {l}, Col {c}");
        if !sel.is_empty() {
            let (s, e) = (ops::floor_char(&d.text, sel.start()), ops::floor_char(&d.text, sel.end()));
            lncol.push_str(&format!(" ({} selected)", d.text[s..e].chars().count()));
        }
        app.set_status_lncol(lncol.into());
        app.set_status_indent(if d.use_tabs { format!("Tab Size: {}", d.tab_size) } else { format!("Spaces: {}", d.tab_size) }.into());
        app.set_status_eol(if d.crlf { "CRLF" } else { "LF" }.into());
        app.set_status_lang(d.lang_name().into());
        let g = self.cur;
        if self.groups[g].cursor_line != l - 1 {
            self.groups[g].cursor_line = l - 1;
            self.push_group(g);
        }
    }

    fn refresh_line_info(&mut self) {
        let g = self.cur;
        if let Some(t) = self.active_tab() {
            let n = t.doc.text.bytes().filter(|&b| b == b'\n').count() + 1;
            if self.groups[g].line_count != n {
                self.groups[g].line_count = n;
                self.push_group(g);
            }
        }
    }

    fn refresh_tabs(&self) {
        for g in 0..2 {
            self.refresh_group_tabs(g);
        }
    }

    fn refresh_group_tabs(&self, g: usize) {
        let active = self.active_in(g);
        let items: Vec<TabItem> = self
            .group_tabs(g)
            .into_iter()
            .map(|i| {
                let t = &self.tabs[i];
                let title = t.doc.loc.file_name();
                let dup = self.tabs.iter().filter(|o| o.doc.loc.file_name() == title).count() > 1 && t.preview.is_none();
                let desc = if dup {
                    let p = self.parent_of(&t.doc.loc.path_string());
                    let r = self.rel_path(&p);
                    if r.is_empty() { self.ws_name() } else { r }
                } else {
                    String::new()
                };
                let (glyph, color) = if t.preview.is_some() { ("◫", 0x519aba) } else { (t.doc.lang.glyph, t.doc.lang.color) };
                TabItem {
                    title: title.into(),
                    desc: desc.into(),
                    dirty: t.doc.is_dirty(),
                    active: Some(i) == active,
                    glyph: glyph.into(),
                    glyph_color: lang::color(color),
                }
            })
            .collect();
        let gr = &self.groups[g];
        gr.tabs_m.set_vec(items);
        let crumbs: Vec<Crumb> = match active.map(|i| &self.tabs[i]) {
            None => Vec::new(),
            Some(t) => {
                let full = match &t.preview {
                    Some(p) => p.source.path_string(),
                    None => t.doc.loc.path_string(),
                };
                let rel = if matches!(t.doc.loc, Where::Untitled(_)) { full } else { self.rel_path(&full) };
                let parts: Vec<&str> = rel.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
                let n = parts.len();
                parts
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let last = i + 1 == n;
                        Crumb {
                            label: (*p).into(),
                            glyph: if last { t.doc.lang.glyph.into() } else { SharedString::new() },
                            glyph_color: lang::color(t.doc.lang.color),
                        }
                    })
                    .collect()
            }
        };
        gr.crumbs_m.set_vec(crumbs);
        self.push_group(g);
    }

    fn refresh_tree(&self) {
        let sel = self.tree.selected;
        let rows: Vec<TreeRow> = self
            .tree
            .visible
            .iter()
            .map(|&i| {
                let n = &self.tree.nodes[i];
                let l = lang::detect(&n.name);
                TreeRow {
                    name: n.name.as_str().into(),
                    depth: n.depth.saturating_sub(1) as i32,
                    is_dir: n.is_dir,
                    expanded: n.expanded,
                    glyph: if n.is_dir { SharedString::new() } else { l.glyph.into() },
                    glyph_color: lang::color(l.color),
                    selected: sel == Some(i),
                    loading: n.loading,
                }
            })
            .collect();
        self.m.tree.set_vec(rows);
    }

    fn reload_hosts(&mut self) {
        self.hosts = ssh_config::load_hosts();
        let connected = self.remote.host.as_ref().filter(|_| self.remote.connected).map(|h| h.alias.clone());
        self.m.hosts.set_vec(
            self.hosts
                .iter()
                .map(|h| HostRow {
                    name: h.alias.as_str().into(),
                    detail: h.detail().into(),
                    connected: connected.as_deref() == Some(h.alias.as_str()),
                })
                .collect::<Vec<_>>(),
        );
    }

    // ------------------------------------------------------------------ editor <-> widget

    /// Put the active document of the current group into its widget (tab switch / load).
    fn load_editor(&mut self) {
        let g = self.cur;
        self.sync_active();
        self.load_group(g);
        let app = self.ui();
        if self.active.is_none() {
            app.invoke_focus_root();
        }
        self.refresh_status();
        self.refresh_title();
        if self.find.open {
            self.find_count();
        }
    }

    /// Put group `g`'s active tab into that group's widget.
    fn load_group(&mut self, g: usize) {
        let app = self.ui();
        self.refresh_tabs();
        let gi = g as i32;
        match self.active_in(g) {
            Some(i) if self.tabs[i].preview.is_none() => {
                let t = &self.tabs[i];
                app.invoke_set_text(gi, t.doc.text.as_str().into());
                let (sel, scroll) = (t.doc.sel, t.scroll);
                let n = t.doc.text.bytes().filter(|&b| b == b'\n').count() + 1;
                app.invoke_set_selection(gi, sel.anchor as i32, sel.cursor as i32);
                app.invoke_set_scroll(gi, scroll.0, scroll.1);
                let (l, _) = ops::line_col(&t.doc.text, sel.cursor.min(t.doc.text.len()), t.doc.tab_size);
                self.groups[g].line_count = n;
                self.groups[g].cursor_line = l - 1;
                self.groups[g].blocks_m.set_vec(Vec::new());
            }
            Some(_) => {
                app.invoke_set_text(gi, "".into());
                self.render_preview(g);
            }
            None => {
                app.invoke_set_text(gi, "".into());
                self.groups[g].blocks_m.set_vec(Vec::new());
            }
        }
        self.refresh_visible(g);
        self.schedule_highlight();
        self.push_group(g);
    }

    /// Our own edit was applied to the active doc; push it to the widget.
    fn push_text(&mut self, sel: Sel) {
        let app = self.ui();
        let Some(i) = self.active else { return };
        app.invoke_set_text(self.g(), self.tabs[i].doc.text.as_str().into());
        app.invoke_set_selection(self.g(), sel.anchor as i32, sel.cursor as i32);
        self.tabs[i].doc.sel = sel;
        self.after_text_change();
    }

    fn after_text_change(&mut self) {
        self.refresh_line_info();
        self.refresh_status();
        self.refresh_visible(self.cur);
        self.schedule_highlight();
        self.schedule_previews();
        let was_dirty: Vec<bool> = (0..2).flat_map(|g| self.groups[g].tabs_m.iter().map(|t| t.dirty).collect::<Vec<_>>()).collect();
        let now_dirty: Vec<bool> = (0..2).flat_map(|g| self.group_tabs(g).into_iter().map(|i| self.tabs[i].doc.is_dirty())).collect();
        if was_dirty != now_dirty {
            self.refresh_tabs();
            self.refresh_title();
        }
        if self.find.open {
            self.find_count();
        }
    }

    // ------------------------------------------------------------------ syntax highlighting

    pub(crate) fn schedule_highlight(&mut self) {
        let stale = (0..2).any(|g| {
            self.active_in(g).is_some_and(|i| {
                let d = &self.tabs[i].doc;
                d.grammar.is_some() && d.spans_rev != d.rev && self.tabs[i].preview.is_none()
            })
        });
        if !stale {
            return;
        }
        self.hl_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(40), || {
            with_state(|s| s.run_highlight());
        });
    }

    fn run_highlight(&mut self) {
        for g in 0..2 {
            let Some(i) = self.active_in(g) else { continue };
            if self.tabs[i].preview.is_some() {
                continue;
            }
            let d = &mut self.tabs[i].doc;
            let Some(gr) = d.grammar else { continue };
            if d.spans_rev == d.rev {
                continue;
            }
            match crate::syntax::highlight(gr, &d.text) {
                Ok(spans) => {
                    d.spans = spans;
                    d.spans_rev = d.rev;
                }
                Err(_) => {
                    d.grammar = None;
                    d.spans.clear();
                    for e in crate::syntax::take_load_errors() {
                        self.toast(2, format!("Syntax highlighting is unavailable: {e}"));
                    }
                }
            }
            self.refresh_visible(g);
        }
    }

    pub fn editor_viewport(&mut self, g: usize, first: usize, rows: usize) {
        self.groups[g].view_first = first;
        self.groups[g].view_rows = rows.max(1);
        self.refresh_visible(g);
    }

    /// Rebuild the colored overlay for the visible lines of group `g` (plus a small margin).
    fn refresh_visible(&mut self, g: usize) {
        let doc = self
            .active_in(g)
            .map(|i| &self.tabs[i])
            .filter(|t| t.preview.is_none())
            .map(|t| &t.doc)
            .filter(|d| d.grammar.is_some() && d.message.is_empty());
        let gr = &self.groups[g];
        let Some(d) = doc else {
            gr.hl.set_vec(Vec::new());
            if gr.hl_active {
                self.groups[g].hl_active = false;
                self.push_group(g);
            }
            return;
        };
        let first = gr.view_first.saturating_sub(2);
        let last = gr.view_first + gr.view_rows + 2;
        let text = d.text.as_str();
        let mut ls = ops::line_offset(text, first);
        let mut idx = ops::line_index(text, ls);
        let mut lines = Vec::with_capacity(last - first);
        while idx <= last {
            let le = ops::line_end(text, ls);
            let spans: Vec<HlSpan> = crate::syntax::line_runs(text, ls, le, &d.spans)
                .into_iter()
                .map(|(t, c)| HlSpan { text: t.into(), color: lang::color(c) })
                .collect();
            lines.push(HlLine { line: idx as i32, spans: ModelRc::from(Rc::new(VecModel::from(spans))) });
            if le >= text.len() {
                break;
            }
            ls = le + 1;
            idx += 1;
        }
        gr.hl.set_vec(lines);
        if !gr.hl_active {
            self.groups[g].hl_active = true;
            self.push_group(g);
        }
    }

    fn save_view_state(&mut self) {
        let app = self.ui();
        if let Some(i) = self.active {
            if self.tabs[i].doc.message.is_empty() && self.tabs[i].preview.is_none() {
                self.tabs[i].scroll = (app.invoke_editor_scroll_x(self.g()), app.invoke_editor_scroll_y(self.g()));
                self.tabs[i].doc.sel = self.widget_sel();
            }
        }
    }

    pub fn editor_edited(&mut self, text: &str) {
        let sel = self.widget_sel();
        let Some(i) = self.active else { return };
        self.tabs[i].doc.sync_from_widget(text, sel);
        self.after_text_change();
    }

    pub fn editor_cursor_moved(&mut self, cursor: usize, anchor: usize) {
        let app = self.ui();
        let Some(i) = self.active else { return };
        let d = &mut self.tabs[i].doc;
        // text not synced yet means this move belongs to an edit in flight
        if app.invoke_editor_text(self.cur as i32).len() == d.text.len() && d.sel.cursor != cursor {
            d.break_merge();
        }
        d.sel = Sel::new(anchor, cursor);
        self.refresh_status();
    }

    fn apply_edit(&mut self, e: Edit) {
        let Some(i) = self.active else { return };
        self.tabs[i].doc.sel = self.widget_sel();
        self.tabs[i].doc.apply(&e);
        self.push_text(e.sel);
    }

    fn set_sel(&mut self, sel: Sel) {
        let app = self.ui();
        app.invoke_set_selection(self.g(), sel.anchor as i32, sel.cursor as i32);
        if let Some(i) = self.active {
            self.tabs[i].doc.sel = sel;
            self.tabs[i].doc.break_merge();
        }
        self.refresh_status();
    }

    fn center_line(&self, line0: usize) {
        let app = self.ui();
        let lh = app.get_line_h();
        let vis = app.invoke_editor_visible_height(self.g());
        let y = -(line0 as f32 * lh - vis / 2.0 + lh / 2.0);
        self.set_scroll_y(y.min(0.0));
    }

    /// Editor-specific keys. Returns true when handled (the widget must not handle it).
    pub fn editor_key(&mut self, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        let Some(i) = self.active else { return false };
        if self.tabs[i].doc.read_only || self.tabs[i].preview.is_some() {
            return false;
        }
        let sel = self.widget_sel();
        let text: &str = &shortcut_key(text, ctrl, alt);
        let lower = text.to_lowercase();
        let k = lower.as_str();
        let d = &self.tabs[i].doc;
        let (tab, unit, use_tabs) = (d.tab_size, d.indent_unit(), d.use_tabs);
        let t = d.text.as_str();
        let multi_line = !sel.is_empty() && t[sel.start()..sel.end()].contains('\n');

        let edit: Option<Edit> = if (is_key(text, Key::Tab) && shift) || is_key(text, Key::Backtab) {
            if ctrl { return false; }
            Some(ops::outdent_lines(t, sel, tab))
        } else if is_key(text, Key::Tab) {
            if ctrl || alt {
                return false;
            }
            Some(if multi_line { ops::indent_lines(t, sel, &unit) } else { ops::tab_insert(t, sel, use_tabs, tab) })
        } else if is_key(text, Key::Return) {
            match (ctrl, shift) {
                (false, _) if !alt => Some(ops::newline(t, sel, &unit)),
                (true, false) => Some(ops::insert_line(t, sel, false)),
                (true, true) => Some(ops::insert_line(t, sel, true)),
                _ => None,
            }
        } else if is_key(text, Key::Backspace) && !ctrl && !alt && !shift {
            ops::backspace(t, sel, tab)
        } else if (is_key(text, Key::UpArrow) || is_key(text, Key::DownArrow)) && alt && !ctrl {
            let up = is_key(text, Key::UpArrow);
            if shift { Some(ops::copy_lines(t, sel, up)) } else { ops::move_lines(t, sel, up) }
        } else if (is_key(text, Key::UpArrow) || is_key(text, Key::DownArrow)) && ctrl && !alt && !shift {
            // Ctrl+Up/Down scrolls one line without moving the cursor
            let app = self.ui();
            let lh = app.get_line_h();
            let dy = if is_key(text, Key::UpArrow) { lh } else { -lh };
            self.set_scroll_y((self.scroll_y() + dy).min(0.0));
            return true;
        } else if is_key(text, Key::Home) && !ctrl && !alt {
            let s = ops::smart_home(t, sel, shift);
            self.set_sel(s);
            return true;
        } else if ctrl && !alt {
            match (k, shift) {
                ("k", true) => Some(ops::delete_lines(t, sel)),
                ("/", false) | ("7", false) => ops::toggle_line_comment(t, sel, d.comment_token()),
                ("]", false) => Some(ops::indent_lines(t, sel, &unit)),
                ("[", false) => Some(ops::outdent_lines(t, sel, tab)),
                ("l", false) => {
                    let s = ops::select_line(t, sel);
                    self.set_sel(s);
                    return true;
                }
                ("d", false) => {
                    self.next_occurrence(sel);
                    return true;
                }
                ("z", false) => {
                    self.undo();
                    return true;
                }
                ("y", false) | ("z", true) => {
                    self.redo();
                    return true;
                }
                ("c", false) if sel.is_empty() => {
                    // copy the whole line
                    let ls = ops::line_start(t, sel.cursor);
                    let le = (ops::line_end(t, sel.cursor) + 1).min(t.len());
                    let app = self.ui();
                    let g = self.g();
                    app.invoke_set_selection(g, ls as i32, le as i32);
                    app.invoke_editor_copy(g);
                    app.invoke_set_selection(g, sel.anchor as i32, sel.cursor as i32);
                    return true;
                }
                ("x", false) if sel.is_empty() => {
                    let ls = ops::line_start(t, sel.cursor);
                    let le = (ops::line_end(t, sel.cursor) + 1).min(t.len());
                    let g = self.g();
                    self.ui().invoke_set_selection(g, ls as i32, le as i32);
                    let w = self.app.clone();
                    later(move || {
                        if let Some(a) = w.upgrade() {
                            a.invoke_editor_cut(g);
                        }
                    });
                    return true;
                }
                _ => None,
            }
        } else if !ctrl && !alt {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if "([{\"'`)]}".contains(c) => ops::type_char(t, sel, c),
                _ => None,
            }
        } else {
            None
        };
        match edit {
            Some(e) => {
                self.apply_edit(e);
                true
            }
            None => false,
        }
    }

    fn next_occurrence(&mut self, sel: Sel) {
        let Some(i) = self.active else { return };
        let t = &self.tabs[i].doc.text;
        if sel.is_empty() {
            let (s, e) = ops::word_at(t, sel.cursor);
            if s < e {
                self.set_sel(Sel::new(s, e));
            }
            return;
        }
        let needle = t[sel.start()..sel.end()].to_string();
        let matches = ops::find_all(t, &needle, true, false, 100_000);
        let next = matches.iter().find(|m| m.0 >= sel.end()).or(matches.first()).copied();
        if let Some((s, e)) = next {
            self.set_sel(Sel::new(s, e));
        }
    }

    fn undo(&mut self) {
        let Some(i) = self.active else { return };
        if let Some(s) = self.tabs[i].doc.undo() {
            self.push_text(s);
        }
    }

    fn redo(&mut self) {
        let Some(i) = self.active else { return };
        if let Some(s) = self.tabs[i].doc.redo() {
            self.push_text(s);
        }
    }

    pub fn gutter_clicked(&mut self, line: i32, shift: bool) {
        let Some(i) = self.active else { return };
        let t = &self.tabs[i].doc.text;
        let line = line.max(0) as usize;
        let s = ops::line_offset(t, line);
        if ops::line_index(t, s) != line && line > 0 {
            return; // below the last line
        }
        let e = (ops::line_end(t, s) + 1).min(t.len());
        let sel = if shift {
            let cur = self.widget_sel();
            if s < cur.anchor { Sel::new(cur.anchor.max(e), s) } else { Sel::new(cur.start(), e) }
        } else {
            Sel::new(s, e)
        };
        self.focus_ed();
        self.set_sel(sel);
    }
}


mod ext;
mod files;
mod input;
mod search;
#[cfg(feature = "screenshot")]
mod shot;


#[cfg(test)]
mod tests {
    use super::shortcut_key;

    #[test]
    fn hangul_ime_shortcuts_map_to_latin_keys() {
        assert_eq!(shortcut_key("ㄴ", true, false), "s");
        assert_eq!(shortcut_key("ㅆ", true, false), "t");
        assert_eq!(shortcut_key("ㅏ", true, false), "k");
        assert_eq!(shortcut_key("ㄴ", false, false), "ㄴ");
        assert_eq!(shortcut_key("s", true, false), "s");
    }
}
