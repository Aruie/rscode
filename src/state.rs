//! Application state and all UI behavior. Lives on the UI thread in a thread_local.

use crate::commands::{self, Entry, COMMANDS};
use crate::doc::{Doc, Where};
use crate::lang;
use crate::remote::{self, Event, Req};
use crate::ssh_config::{self, HostEntry};
use crate::text_ops::{self as ops, Edit, Sel};
use crate::tree::Tree;
use crate::{App, Crumb, HostRow, MenuItemData, QuickItem, TabItem, Toast, TreeRow};
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

const MAX_FILE: u64 = 20 * 1024 * 1024;
const BINARY_MSG: &str = "The file is not displayed in the text editor because it is either binary or uses an unsupported text encoding.";
const LARGE_MSG: &str = "The file is not displayed in the text editor because it is too large (over 20 MB).";

pub struct Tab {
    id: u64,
    doc: Doc,
    scroll: (f32, f32),
    close_after_save: bool,
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
    key: &'static str,
    action: QAction,
}

impl QRow {
    fn text(label: impl Into<String>, action: QAction) -> Self {
        QRow { label: label.into(), desc: String::new(), glyph: "", color: 0, key: "", action }
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
    tabs: Rc<VecModel<TabItem>>,
    quick: Rc<VecModel<QuickItem>>,
    toasts: Rc<VecModel<Toast>>,
    crumbs: Rc<VecModel<Crumb>>,
    hosts: Rc<VecModel<HostRow>>,
    popup: Rc<VecModel<MenuItemData>>,
}

pub struct State {
    app: Weak<App>,
    m: Models,
    tabs: Vec<Tab>,
    active: Option<usize>,
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
}

fn rows_model<T: Clone + 'static>(m: &Rc<VecModel<T>>) -> ModelRc<T> {
    ModelRc::from(m.clone())
}

impl State {
    pub fn new(app: &App) -> Self {
        let m = Models {
            tree: Rc::new(VecModel::default()),
            tabs: Rc::new(VecModel::default()),
            quick: Rc::new(VecModel::default()),
            toasts: Rc::new(VecModel::default()),
            crumbs: Rc::new(VecModel::default()),
            hosts: Rc::new(VecModel::default()),
            popup: Rc::new(VecModel::default()),
        };
        app.set_tree_rows(rows_model(&m.tree));
        app.set_tabs(rows_model(&m.tabs));
        app.set_quick_items(rows_model(&m.quick));
        app.set_toasts(rows_model(&m.toasts));
        app.set_crumbs(rows_model(&m.crumbs));
        app.set_hosts(rows_model(&m.hosts));
        app.set_popup_items(rows_model(&m.popup));
        let mut s = State {
            app: app.as_weak(),
            m,
            tabs: Vec::new(),
            active: None,
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
        };
        s.reload_hosts();
        s.refresh_title();
        s.refresh_status();
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

    fn widget_sel(&self) -> Sel {
        let app = self.ui();
        Sel::new(app.get_editor_anchor().max(0) as usize, app.get_editor_cursor().max(0) as usize)
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

    fn refresh_status(&self) {
        let app = self.ui();
        let Some(t) = self.active_tab() else {
            app.set_editor_visible(false);
            return;
        };
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
        app.set_status_lang(d.lang.name.into());
        app.set_cursor_line((l - 1) as i32);
    }

    fn refresh_line_info(&self) {
        let app = self.ui();
        if let Some(t) = self.active_tab() {
            let n = t.doc.text.bytes().filter(|&b| b == b'\n').count() + 1;
            app.set_line_count(n as i32);
            app.set_line_digits(n.to_string().len() as i32);
        }
    }

    fn refresh_tabs(&self) {
        let items: Vec<TabItem> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let title = t.doc.loc.file_name();
                let dup = self.tabs.iter().filter(|o| o.doc.loc.file_name() == title).count() > 1;
                let desc = if dup {
                    let p = self.parent_of(&t.doc.loc.path_string());
                    let r = self.rel_path(&p);
                    if r.is_empty() { self.ws_name() } else { r }
                } else {
                    String::new()
                };
                TabItem {
                    title: title.into(),
                    desc: desc.into(),
                    dirty: t.doc.is_dirty(),
                    active: Some(i) == self.active,
                    glyph: t.doc.lang.glyph.into(),
                    glyph_color: lang::color(t.doc.lang.color),
                }
            })
            .collect();
        self.m.tabs.set_vec(items);
        // breadcrumbs
        let crumbs: Vec<Crumb> = match self.active_tab() {
            None => Vec::new(),
            Some(t) => {
                let full = t.doc.loc.path_string();
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
        self.m.crumbs.set_vec(crumbs);
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

    /// Put the active document into the text widget (tab switch / load).
    fn load_editor(&mut self) {
        let app = self.ui();
        self.refresh_tabs();
        let Some(i) = self.active else {
            app.set_editor_visible(false);
            app.set_editor_message("".into());
            app.set_editor_text("".into());
            self.refresh_title();
            app.invoke_focus_root();
            return;
        };
        let t = &self.tabs[i];
        let has_text = t.doc.message.is_empty();
        app.set_editor_message(t.doc.message.as_str().into());
        app.set_editor_visible(has_text);
        app.set_editor_read_only(t.doc.read_only);
        app.set_editor_text(t.doc.text.as_str().into());
        let sel = t.doc.sel;
        let scroll = t.scroll;
        app.invoke_set_selection(sel.anchor as i32, sel.cursor as i32);
        app.set_editor_scroll_x(scroll.0);
        app.set_editor_scroll_y(scroll.1);
        self.refresh_line_info();
        self.refresh_status();
        self.refresh_title();
        if self.find.open {
            self.find_count();
        }
    }

    /// Our own edit was applied to the active doc; push it to the widget.
    fn push_text(&mut self, sel: Sel) {
        let app = self.ui();
        let Some(i) = self.active else { return };
        app.set_editor_text(self.tabs[i].doc.text.as_str().into());
        app.invoke_set_selection(sel.anchor as i32, sel.cursor as i32);
        self.tabs[i].doc.sel = sel;
        self.after_text_change();
    }

    fn after_text_change(&mut self) {
        self.refresh_line_info();
        self.refresh_status();
        let was_dirty: Vec<bool> = self.m.tabs.iter().map(|t| t.dirty).collect();
        let now_dirty: Vec<bool> = self.tabs.iter().map(|t| t.doc.is_dirty()).collect();
        if was_dirty != now_dirty {
            self.refresh_tabs();
            self.refresh_title();
        }
        if self.find.open {
            self.find_count();
        }
    }

    fn save_view_state(&mut self) {
        let app = self.ui();
        if let Some(i) = self.active {
            if self.tabs[i].doc.message.is_empty() {
                self.tabs[i].scroll = (app.get_editor_scroll_x(), app.get_editor_scroll_y());
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
        if app.get_editor_text().len() == d.text.len() && d.sel.cursor != cursor {
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
        app.invoke_set_selection(sel.anchor as i32, sel.cursor as i32);
        if let Some(i) = self.active {
            self.tabs[i].doc.sel = sel;
            self.tabs[i].doc.break_merge();
        }
        self.refresh_status();
    }

    fn center_line(&self, line0: usize) {
        let app = self.ui();
        let lh = app.get_line_h();
        let vis = app.get_editor_visible_height();
        let y = -(line0 as f32 * lh - vis / 2.0 + lh / 2.0);
        app.set_editor_scroll_y(y.min(0.0));
    }

    /// Editor-specific keys. Returns true when handled (the widget must not handle it).
    pub fn editor_key(&mut self, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        let Some(i) = self.active else { return false };
        if self.tabs[i].doc.read_only {
            return false;
        }
        let sel = self.widget_sel();
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
            app.set_editor_scroll_y((app.get_editor_scroll_y() + dy).min(0.0));
            return true;
        } else if is_key(text, Key::Home) && !ctrl && !alt {
            let s = ops::smart_home(t, sel, shift);
            self.set_sel(s);
            return true;
        } else if ctrl && !alt {
            match (k, shift) {
                ("k", true) => Some(ops::delete_lines(t, sel)),
                ("/", false) | ("7", false) => ops::toggle_line_comment(t, sel, d.lang.comment),
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
                    app.invoke_set_selection(ls as i32, le as i32);
                    app.invoke_editor_copy();
                    app.invoke_set_selection(sel.anchor as i32, sel.cursor as i32);
                    return true;
                }
                ("x", false) if sel.is_empty() => {
                    let ls = ops::line_start(t, sel.cursor);
                    let le = (ops::line_end(t, sel.cursor) + 1).min(t.len());
                    self.ui().invoke_set_selection(ls as i32, le as i32);
                    let w = self.app.clone();
                    later(move || {
                        if let Some(a) = w.upgrade() {
                            a.invoke_editor_cut();
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
        self.ui().invoke_focus_editor();
        self.set_sel(sel);
    }
}


mod files;
mod input;
#[cfg(feature = "screenshot")]
mod shot;
