//! Tabs, files, workspace folder, explorer actions and the SSH connection.

use super::*;

fn decode(bytes: Vec<u8>) -> Result<String, &'static str> {
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err(BINARY_MSG);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).map(|b| b.to_vec()).unwrap_or(bytes);
    String::from_utf8(bytes).map_err(|_| BINARY_MSG)
}

impl State {
    // ------------------------------------------------------------------ tabs

    fn find_tab(&self, loc: &Where) -> Option<usize> {
        self.tabs.iter().position(|t| &t.doc.loc == loc)
    }

    fn add_tab(&mut self, doc: Doc) -> usize {
        self.save_view_state();
        let id = self.next_id;
        self.next_id += 1;
        let insert_at = self.active.map(|a| a + 1).unwrap_or(self.tabs.len());
        self.tabs.insert(insert_at, Tab { id, doc, scroll: (0.0, 0.0), close_after_save: false });
        self.activate(insert_at);
        insert_at
    }

    pub fn activate(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.active != Some(idx) {
            self.save_view_state();
        }
        self.active = Some(idx);
        let id = self.tabs[idx].id;
        self.mru.retain(|&m| m != id);
        self.mru.insert(0, id);
        self.load_editor();
    }

    pub fn tab_clicked(&mut self, idx: usize) {
        self.activate(idx);
        self.ui().invoke_focus_editor();
    }

    pub fn new_untitled(&mut self) {
        self.untitled += 1;
        let doc = Doc::new(Where::Untitled(self.untitled), String::new());
        self.add_tab(doc);
        self.ui().invoke_focus_editor();
    }

    pub fn request_close_tab(&mut self, idx: usize) {
        let Some(t) = self.tabs.get(idx) else { return };
        if t.doc.is_dirty() {
            let name = t.doc.loc.file_name();
            let id = t.id;
            self.show_dialog(
                Dialog::CloseTab(id),
                &format!("Do you want to save the changes you made to {name}?"),
                "Your changes will be lost if you don't save them.",
                &["Save", "Don't Save", "Cancel"],
            );
        } else {
            self.close_tab(idx);
        }
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if Some(idx) == self.active {
            self.save_view_state();
        }
        let t = self.tabs.remove(idx);
        if !matches!(t.doc.loc, Where::Untitled(_)) {
            self.closed.retain(|l| l != &t.doc.loc);
            self.closed.push(t.doc.loc.clone());
        }
        self.mru.retain(|&m| m != t.id);
        if self.tabs.is_empty() {
            self.active = None;
        } else {
            // like VS Code: go to the most recently used remaining tab
            let next = self.mru.first().and_then(|&id| self.tab_index(id)).unwrap_or(idx.min(self.tabs.len() - 1));
            self.active = Some(next);
        }
        self.load_editor();
        if self.active.is_some() {
            self.ui().invoke_focus_editor();
        }
    }

    pub fn close_all_tabs(&mut self) {
        self.tabs.clear();
        self.active = None;
        self.mru.clear();
        self.load_editor();
    }

    pub fn reopen_closed(&mut self) {
        if let Some(loc) = self.closed.pop() {
            match loc {
                Where::Local(p) => self.open_local(&p),
                Where::Remote(p) => self.open_remote(p),
                Where::Untitled(_) => {}
            }
        }
    }

    pub fn cycle_tab(&mut self, delta: isize) {
        let n = self.tabs.len();
        if n < 2 {
            return;
        }
        let cur = self.active.unwrap_or(0) as isize;
        self.activate(((cur + delta).rem_euclid(n as isize)) as usize);
        self.ui().invoke_focus_editor();
    }

    /// Ctrl+Tab: previous editor in most-recently-used order.
    pub fn mru_switch(&mut self, back: bool) {
        if self.mru.len() < 2 {
            return;
        }
        let id = if back { *self.mru.last().unwrap() } else { self.mru[1] };
        if let Some(i) = self.tab_index(id) {
            self.activate(i);
            self.ui().invoke_focus_editor();
        }
    }

    // ------------------------------------------------------------------ open

    pub fn open_local(&mut self, path: &Path) {
        let loc = Where::Local(path.to_path_buf());
        if let Some(i) = self.find_tab(&loc) {
            self.activate(i);
            return;
        }
        let doc = match std::fs::metadata(path) {
            Err(e) => {
                self.toast(2, format!("Unable to open '{}': {e}", loc.file_name()));
                return;
            }
            Ok(m) if m.len() > MAX_FILE => Doc::placeholder(loc, LARGE_MSG),
            Ok(_) => match std::fs::read(path) {
                Err(e) => {
                    self.toast(2, format!("Unable to open '{}': {e}", loc.file_name()));
                    return;
                }
                Ok(bytes) => match decode(bytes) {
                    Ok(text) => Doc::new(loc, text),
                    Err(msg) => Doc::placeholder(loc, msg),
                },
            },
        };
        self.add_tab(doc);
    }

    pub fn open_remote(&mut self, path: String) {
        let loc = Where::Remote(path.clone());
        if let Some(i) = self.find_tab(&loc) {
            self.activate(i);
            return;
        }
        if self.send(Req::ReadFile { path }) {
            self.add_tab(Doc::placeholder(loc, "Loading..."));
        }
    }

    pub fn open_path(&mut self, path: &str) {
        match self.ws {
            Ws::Remote(_) => self.open_remote(path.to_string()),
            _ => self.open_local(Path::new(path)),
        }
    }

    pub(super) fn on_remote_file(&mut self, path: String, result: Result<Vec<u8>, String>) {
        let loc = Where::Remote(path);
        let Some(i) = self.find_tab(&loc) else { return };
        match result {
            Ok(bytes) => {
                let doc = match decode(bytes) {
                    Ok(text) => Doc::new(loc, text),
                    Err(msg) => Doc::placeholder(loc, msg),
                };
                self.tabs[i].doc = doc;
            }
            Err(e) => {
                self.tabs[i].doc.message = format!("Unable to open '{}': {e}", loc.file_name());
            }
        }
        if self.active == Some(i) {
            self.load_editor();
            self.ui().invoke_focus_editor();
            self.apply_reveal();
        } else {
            self.refresh_tabs();
        }
    }

    pub fn revert(&mut self) {
        let Some(i) = self.active else { return };
        match self.tabs[i].doc.loc.clone() {
            Where::Local(p) => {
                if let Ok(bytes) = std::fs::read(&p) {
                    if let Ok(text) = decode(bytes) {
                        let sel = self.tabs[i].doc.sel;
                        self.tabs[i].doc = Doc::new(Where::Local(p), text);
                        let len = self.tabs[i].doc.text.len();
                        self.tabs[i].doc.sel = Sel::new(sel.anchor.min(len), sel.cursor.min(len));
                        self.load_editor();
                    }
                }
            }
            Where::Remote(p) => {
                if self.send(Req::ReadFile { path: p }) {
                    self.tabs[i].doc.message = "Loading...".into();
                    self.load_editor();
                }
            }
            Where::Untitled(_) => {}
        }
    }

    // ------------------------------------------------------------------ save

    /// Returns false if the save is asynchronous or failed.
    fn save_tab(&mut self, idx: usize) -> bool {
        if Some(idx) == self.active {
            self.save_view_state();
        }
        let t = &self.tabs[idx];
        if !t.doc.message.is_empty() {
            return true;
        }
        match t.doc.loc.clone() {
            Where::Untitled(_) => {
                let id = t.id;
                self.save_as(id);
                false
            }
            Where::Local(p) => {
                let hash = t.doc.current_hash();
                match std::fs::write(&p, t.doc.bytes_for_save()) {
                    Ok(()) => {
                        self.tabs[idx].doc.mark_saved(hash);
                        self.refresh_tabs();
                        self.refresh_title();
                        true
                    }
                    Err(e) => {
                        self.toast(2, format!("Failed to save '{}': {e}", t.doc.loc.file_name()));
                        self.save_failed = true;
                        false
                    }
                }
            }
            Where::Remote(path) => {
                let hash = t.doc.current_hash();
                let data = t.doc.bytes_for_save();
                if self.send(Req::WriteFile { path, data, hash }) {
                    self.pending_saves += 1;
                } else {
                    self.save_failed = true;
                }
                false
            }
        }
    }

    pub fn save_active(&mut self) {
        if let Some(i) = self.active {
            self.save_tab(i);
        }
    }

    pub fn save_all(&mut self) {
        for i in 0..self.tabs.len() {
            if self.tabs[i].doc.is_dirty() {
                self.save_tab(i);
            }
        }
    }

    pub(super) fn on_written(&mut self, path: String, hash: u64, result: Result<(), String>) {
        self.pending_saves = self.pending_saves.saturating_sub(1);
        let loc = Where::Remote(path);
        match result {
            Ok(()) => {
                if let Some(i) = self.find_tab(&loc) {
                    self.tabs[i].doc.mark_saved(hash);
                    if self.tabs[i].close_after_save && !self.tabs[i].doc.is_dirty() {
                        self.close_tab(i);
                    }
                }
                self.refresh_tabs();
                self.refresh_title();
            }
            Err(e) => {
                self.save_failed = true;
                self.toast(2, format!("Failed to save '{}': {e}", loc.file_name()));
            }
        }
        if self.pending_saves == 0 {
            if let Some(then) = self.pending_then.take() {
                if !self.save_failed {
                    self.run_then(then);
                }
            }
        }
    }

    pub fn save_as(&mut self, id: u64) {
        let Some(idx) = self.tab_index(id) else { return };
        if matches!(self.ws, Ws::Remote(_)) || matches!(self.tabs[idx].doc.loc, Where::Remote(_)) {
            let start = match &self.tabs[idx].doc.loc {
                Where::Remote(p) => p.clone(),
                _ => self.join(&self.ws_root().unwrap_or_else(|| self.remote.home.clone()), &self.tabs[idx].doc.loc.file_name()),
            };
            self.open_browse(BrowsePurpose::SaveAs(id), start);
            return;
        }
        let dir = self.ws_root().map(PathBuf::from);
        let name = self.tabs[idx].doc.loc.file_name();
        let name = if name.starts_with("Untitled-") { format!("{name}.txt") } else { name };
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new().set_file_name(&name);
            if let Some(dir) = dir {
                d = d.set_directory(dir);
            }
            if let Some(p) = d.save_file() {
                let _ = slint::invoke_from_event_loop(move || {
                    with_state(|s| s.finish_save_as(id, Where::Local(p)));
                });
            }
        });
    }

    pub(super) fn finish_save_as(&mut self, id: u64, loc: Where) {
        let Some(idx) = self.tab_index(id) else { return };
        if let Some(other) = self.find_tab(&loc) {
            if other != idx {
                self.tabs.remove(other);
            }
        }
        let Some(idx) = self.tab_index(id) else { return };
        let t = &mut self.tabs[idx];
        t.doc.lang = lang::detect(&loc.file_name());
        t.doc.loc = loc;
        t.doc.detect_grammar();
        t.doc.read_only = false;
        self.active = self.tab_index(id).or(self.active);
        self.save_tab(self.tab_index(id).unwrap());
        self.refresh_tabs();
        self.refresh_status();
        self.refresh_title();
        if let Some(parent) = self.tabs[self.tab_index(id).unwrap()].doc.loc.path_string().rsplit_once(['/', '\\']).map(|p| p.0.to_string()) {
            self.refresh_dir(&parent);
        }
    }

    // ------------------------------------------------------------------ dialogs

    pub(super) fn show_dialog(&mut self, d: Dialog, message: &str, detail: &str, buttons: &[&str]) {
        let app = self.ui();
        app.set_dialog_message(message.into());
        app.set_dialog_detail(detail.into());
        let b: Vec<SharedString> = buttons.iter().map(|s| (*s).into()).collect();
        app.set_dialog_buttons(ModelRc::from(Rc::new(VecModel::from(b))));
        app.set_dialog_open(true);
        self.dialog = Some(d);
    }

    pub fn dialog_result(&mut self, idx: i32) {
        let app = self.ui();
        app.set_dialog_open(false);
        let Some(d) = self.dialog.take() else { return };
        match d {
            Dialog::CloseTab(id) => {
                let Some(i) = self.tab_index(id) else { return };
                match idx {
                    0 => {
                        self.tabs[i].close_after_save = true;
                        if self.save_tab(i) {
                            if let Some(i) = self.tab_index(id) {
                                self.close_tab(i);
                            }
                        }
                    }
                    1 => self.close_tab(i),
                    _ => {}
                }
            }
            Dialog::DiscardAll(then) => match idx {
                0 => {
                    self.save_failed = false;
                    self.save_all();
                    if self.pending_saves == 0 {
                        if !self.save_failed && !self.tabs.iter().any(|t| t.doc.is_dirty()) {
                            self.run_then(then);
                        }
                    } else {
                        self.pending_then = Some(then);
                    }
                }
                1 => self.run_then(then),
                _ => {}
            },
            Dialog::Delete(path, is_dir) => {
                if idx == 0 {
                    self.delete_path(path, is_dir);
                }
            }
        }
        if self.active.is_some() {
            app.invoke_focus_editor();
        } else {
            app.invoke_focus_root();
        }
    }

    /// Ask to save dirty editors (if any) and then do `then`.
    pub(super) fn confirm_then(&mut self, then: Then) {
        let dirty: Vec<String> = self.tabs.iter().filter(|t| t.doc.is_dirty()).map(|t| t.doc.loc.file_name()).collect();
        if dirty.is_empty() {
            self.run_then(then);
            return;
        }
        let msg = if dirty.len() == 1 {
            format!("Do you want to save the changes you made to {}?", dirty[0])
        } else {
            format!("Do you want to save the changes to the following {} files?", dirty.len())
        };
        let detail = if dirty.len() > 1 { dirty.join("\n") } else { "Your changes will be lost if you don't save them.".into() };
        self.show_dialog(Dialog::DiscardAll(then), &msg, &detail, &["Save All", "Don't Save", "Cancel"]);
    }

    fn run_then(&mut self, then: Then) {
        match then {
            Then::Quit => {
                let _ = slint::quit_event_loop();
            }
            Then::CloseEditors => self.close_all_tabs(),
            Then::CloseFolder => {
                self.close_all_tabs();
                self.set_workspace(Ws::None);
            }
            Then::OpenLocal(p) => {
                self.close_all_tabs();
                if self.remote.tx.is_some() {
                    self.disconnect_now();
                }
                self.set_workspace(Ws::Local(p));
            }
            Then::Connect(h) => {
                self.close_all_tabs();
                self.set_workspace(Ws::None);
                self.connect_now(h);
            }
            Then::Disconnect => {
                self.close_all_tabs();
                self.set_workspace(Ws::None);
                self.disconnect_now();
            }
        }
    }

    pub fn request_quit(&mut self) {
        self.confirm_then(Then::Quit);
    }

    // ------------------------------------------------------------------ workspace / explorer

    fn set_workspace(&mut self, ws: Ws) {
        self.ws = ws;
        self.file_index = None;
        self.index_loading = false;
        match self.ws.clone() {
            Ws::None => self.tree.clear(),
            Ws::Local(p) => {
                let s = p.to_string_lossy().into_owned();
                self.tree.reset(self.ws_name(), s.clone());
                self.refresh_dir(&s);
            }
            Ws::Remote(p) => {
                self.tree.reset(self.ws_name(), p.clone());
                self.refresh_dir(&p);
            }
        }
        self.refresh_tree();
        self.refresh_title();
        self.refresh_tabs();
    }

    pub fn open_local_folder_now(&mut self, p: PathBuf) {
        self.set_workspace(Ws::Local(p));
    }

    pub fn open_folder(&mut self) {
        if self.remote.connected {
            let start = self.ws_root().unwrap_or_else(|| self.remote.home.clone());
            self.open_browse(BrowsePurpose::OpenFolder, start);
            return;
        }
        std::thread::spawn(|| {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                let _ = slint::invoke_from_event_loop(move || {
                    with_state(|s| s.confirm_then(Then::OpenLocal(p)));
                });
            }
        });
    }

    pub fn open_file_dialog(&mut self) {
        if self.remote.connected {
            let start = self.ws_root().unwrap_or_else(|| self.remote.home.clone());
            self.open_browse(BrowsePurpose::OpenFile, start);
            return;
        }
        let dir = self.ws_root().map(PathBuf::from);
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new();
            if let Some(dir) = dir {
                d = d.set_directory(dir);
            }
            if let Some(p) = d.pick_file() {
                let _ = slint::invoke_from_event_loop(move || {
                    with_state(|s| {
                        s.open_local(&p);
                        s.ui().invoke_focus_editor();
                    });
                });
            }
        });
    }

    pub fn open_remote_folder(&mut self, path: String) {
        let path = if path.len() > 1 { path.trim_end_matches('/').to_string() } else { path };
        self.close_all_tabs();
        self.set_workspace(Ws::Remote(path));
        self.view = "explorer";
        self.sidebar = true;
        self.sync_sidebar();
    }

    pub(super) fn refresh_dir(&mut self, path: &str) {
        if let Some(i) = self.tree.find(path) {
            self.tree.nodes[i].loading = true;
        }
        match self.ws {
            Ws::Remote(_) => {
                self.send(Req::ReadDir { path: path.to_string() });
            }
            Ws::Local(_) => {
                let entries = match std::fs::read_dir(path) {
                    Ok(rd) => rd
                        .flatten()
                        .map(|e| {
                            let is_dir = e.file_type().map(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())).unwrap_or(false);
                            (e.file_name().to_string_lossy().into_owned(), is_dir)
                        })
                        .collect(),
                    Err(e) => {
                        self.toast(2, format!("Unable to read folder '{path}': {e}"));
                        Vec::new()
                    }
                };
                self.on_dir(path.to_string(), Ok(entries));
            }
            Ws::None => {}
        }
    }

    pub(super) fn on_dir(&mut self, path: String, result: Result<Vec<(String, bool)>, String>) {
        if let QMode::Browse(_) = self.q_mode {
            if self.q_open && path == self.q_browse_dir {
                self.q_browse_entries = result.clone().unwrap_or_default();
                crate::tree::sort_entries(&mut self.q_browse_entries);
                self.browse_rows();
            }
        }
        match result {
            Ok(entries) => {
                let remote = matches!(self.ws, Ws::Remote(_));
                self.tree.set_children(&path, entries, |d, n| {
                    if remote { remote::join(d, n) } else { Path::new(d).join(n).to_string_lossy().into_owned() }
                });
            }
            Err(e) => {
                if let Some(i) = self.tree.find(&path) {
                    self.tree.nodes[i].loading = false;
                }
                if self.tree.find(&path).is_some() {
                    self.toast(2, format!("Unable to read folder '{path}': {e}"));
                }
            }
        }
        self.refresh_tree();
    }

    pub fn tree_clicked(&mut self, row: usize) {
        let Some(&idx) = self.tree.visible.get(row) else { return };
        self.tree.selected = Some(idx);
        let n = &self.tree.nodes[idx];
        if n.is_dir {
            self.toggle_dir(idx);
        } else {
            let p = n.path.clone();
            self.open_path(&p);
            self.refresh_tree();
        }
    }

    fn toggle_dir(&mut self, idx: usize) {
        let n = &mut self.tree.nodes[idx];
        n.expanded = !n.expanded;
        let need_load = n.expanded && !n.loaded;
        let p = n.path.clone();
        self.tree.rebuild();
        if need_load {
            self.refresh_dir(&p);
        }
        self.refresh_tree();
    }

    pub fn tree_key(&mut self, text: &str, ctrl: bool, _shift: bool, alt: bool) -> bool {
        if ctrl || alt {
            return false;
        }
        let vis = &self.tree.visible;
        let pos = self.tree.selected.and_then(|s| vis.iter().position(|&v| v == s));
        if is_key(text, Key::DownArrow) || is_key(text, Key::UpArrow) {
            let n = vis.len();
            if n == 0 {
                return true;
            }
            let next = match pos {
                None => 0,
                Some(p) if is_key(text, Key::DownArrow) => (p + 1).min(n - 1),
                Some(p) => p.saturating_sub(1),
            };
            self.tree.selected = Some(vis[next]);
            self.refresh_tree();
            return true;
        }
        let Some(sel) = self.tree.selected else { return false };
        let node = &self.tree.nodes[sel];
        if is_key(text, Key::RightArrow) {
            if node.is_dir && !node.expanded {
                self.toggle_dir(sel);
            } else if node.is_dir {
                if let Some(&c) = node.children.first() {
                    self.tree.selected = Some(c);
                    self.refresh_tree();
                }
            }
            return true;
        }
        if is_key(text, Key::LeftArrow) {
            if node.is_dir && node.expanded {
                self.toggle_dir(sel);
            } else if let Some(p) = node.parent.filter(|&p| Some(p) != self.tree.root) {
                self.tree.selected = Some(p);
                self.refresh_tree();
            }
            return true;
        }
        if is_key(text, Key::Return) || text == " " {
            if node.is_dir {
                self.toggle_dir(sel);
            } else {
                let p = node.path.clone();
                self.open_path(&p);
                if is_key(text, Key::Return) {
                    self.ui().invoke_focus_editor();
                }
            }
            return true;
        }
        if is_key(text, Key::F2) {
            self.run_command("tree.rename");
            return true;
        }
        if is_key(text, Key::Delete) {
            self.run_command("tree.delete");
            return true;
        }
        false
    }

    pub fn explorer_action(&mut self, action: &str) {
        match action {
            "new-file" => self.run_command("tree.newFile"),
            "new-folder" => self.run_command("tree.newFolder"),
            "refresh" => self.run_command("tree.refresh"),
            "collapse" => {
                self.tree.collapse_all();
                self.refresh_tree();
            }
            "open-folder" => self.open_folder(),
            "open-remote" => self.run_command("remote.connect"),
            _ => {}
        }
    }

    pub(super) fn target_dir(&self) -> Option<String> {
        self.tree.target_dir().map(|i| self.tree.nodes[i].path.clone())
    }

    pub(super) fn create_item(&mut self, dir: String, name: &str, is_dir: bool) {
        let name = name.trim();
        if name.is_empty() || name.contains(['/', '\\']) && !name.split(['/', '\\']).all(|p| !p.is_empty()) {
            return;
        }
        let path = self.join(&dir, name);
        match self.ws {
            Ws::Remote(_) => {
                let req = if is_dir { Req::CreateDir { path } } else { Req::CreateFile { path } };
                self.send(req);
            }
            _ => {
                let p = PathBuf::from(&path);
                let res = if p.exists() {
                    Err(format!("A file or folder {name} already exists at this location."))
                } else if is_dir {
                    std::fs::create_dir_all(&p).map_err(|e| e.to_string())
                } else {
                    p.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&p, b"")).map_err(|e| e.to_string())
                };
                self.on_done(if is_dir { "create-dir" } else { "create-file" }, path, res);
            }
        }
    }

    pub(super) fn rename_item(&mut self, from: String, new_name: &str) {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return;
        }
        let to = self.join(&self.parent_of(&from), new_name);
        if to == from {
            return;
        }
        match self.ws {
            Ws::Remote(_) => {
                self.send(Req::Rename { from, to });
            }
            _ => {
                let res = std::fs::rename(&from, &to).map_err(|e| e.to_string());
                self.on_done("rename", format!("{from}\n{to}"), res);
            }
        }
    }

    pub(super) fn delete_path(&mut self, path: String, is_dir: bool) {
        match self.ws {
            Ws::Remote(_) => {
                self.send(Req::Remove { path, is_dir });
            }
            _ => {
                let res = if is_dir { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) }.map_err(|e| e.to_string());
                self.on_done("remove", path, res);
            }
        }
    }

    pub(super) fn on_done(&mut self, op: &str, path: String, result: Result<(), String>) {
        if let Err(e) = result {
            self.toast(2, e);
            return;
        }
        match op {
            "create-file" | "create-dir" => {
                let parent = self.parent_of(&path);
                self.expand_and_refresh(&parent);
                if op == "create-file" {
                    self.open_path(&path);
                    self.ui().invoke_focus_editor();
                }
            }
            "remove" => {
                let sep = self.sep();
                let under = |p: &str| p == path || p.starts_with(&format!("{path}{sep}"));
                let ids: Vec<u64> = self.tabs.iter().filter(|t| under(&t.doc.loc.path_string())).map(|t| t.id).collect();
                for id in ids {
                    if let Some(i) = self.tab_index(id) {
                        self.close_tab(i);
                    }
                }
                let parent = self.parent_of(&path);
                self.refresh_dir(&parent);
            }
            "rename" => {
                let Some((from, to)) = path.split_once('\n') else { return };
                let sep = self.sep();
                for t in &mut self.tabs {
                    let p = t.doc.loc.path_string();
                    let new = if p == from {
                        Some(to.to_string())
                    } else {
                        p.strip_prefix(&format!("{from}{sep}")).map(|rest| format!("{to}{sep}{rest}"))
                    };
                    if let Some(new) = new {
                        t.doc.loc = match t.doc.loc {
                            Where::Remote(_) => Where::Remote(new),
                            _ => Where::Local(PathBuf::from(new)),
                        };
                        t.doc.lang = lang::detect(&t.doc.loc.file_name());
                        t.doc.detect_grammar();
                    }
                }
                let parent = self.parent_of(from);
                self.refresh_dir(&parent);
                self.refresh_tabs();
                self.refresh_status();
                self.refresh_title();
            }
            _ => {}
        }
        self.file_index = None;
    }

    fn expand_and_refresh(&mut self, dir: &str) {
        if let Some(i) = self.tree.find(dir) {
            self.tree.nodes[i].expanded = true;
            self.tree.rebuild();
        }
        self.refresh_dir(dir);
    }

    // ------------------------------------------------------------------ file index for Quick Open

    pub(super) fn ensure_index(&mut self) {
        if self.file_index.is_some() || self.index_loading {
            return;
        }
        let Some(root) = self.ws_root() else { return };
        self.index_loading = true;
        match self.ws {
            Ws::Remote(_) => {
                if !self.send(Req::Walk { root }) {
                    self.index_loading = false;
                }
            }
            _ => {
                std::thread::spawn(move || {
                    let (files, truncated) = walk_local(Path::new(&root));
                    let _ = slint::invoke_from_event_loop(move || {
                        with_state(|s| s.on_walked(root, files, truncated));
                    });
                });
            }
        }
    }

    pub(super) fn on_walked(&mut self, root: String, files: Vec<String>, truncated: bool) {
        if self.ws_root().as_deref() != Some(root.as_str()) {
            return;
        }
        self.index_loading = false;
        self.file_index = Some(files);
        if truncated {
            self.toast(1, "This folder has many files; Quick Open shows the first 20,000.".into());
        }
        if self.q_open && matches!(self.q_mode, QMode::Nav) {
            let t = self.ui().get_quick_text().to_string();
            self.quick_edited(&t);
        }
    }

    // ------------------------------------------------------------------ SSH

    pub fn connect_host(&mut self, h: HostEntry) {
        if self.remote.tx.is_some() || !self.tabs.is_empty() || self.ws != Ws::None {
            self.confirm_then(Then::Connect(h));
        } else {
            self.connect_now(h);
        }
    }

    fn connect_now(&mut self, h: HostEntry) {
        if self.remote.tx.is_some() {
            self.disconnect_now();
        }
        self.remote.generation += 1;
        let generation = self.remote.generation;
        let emit: remote::Emit = Arc::new(move |ev| {
            let _ = slint::invoke_from_event_loop(move || {
                with_state(|s| s.on_remote(generation, ev));
            });
        });
        self.remote.host = Some(h.clone());
        self.remote.connected = false;
        let tid = self.toast(3, format!("Setting up SSH Host {}: Connecting...", h.alias));
        self.remote.toast = Some(tid);
        self.remote.tx = Some(remote::connect(h, emit));
        self.refresh_title();
    }

    fn disconnect_now(&mut self) {
        if let Some(tx) = self.remote.tx.take() {
            let _ = tx.send(Req::Disconnect);
        }
        self.remote.generation += 1;
        self.remote.connected = false;
        self.remote.host = None;
        if let Some(t) = self.remote.toast.take() {
            self.remote_toast_done(t);
        }
        self.reload_hosts();
        self.refresh_title();
    }

    fn remote_toast_done(&mut self, id: u64) {
        self.remove_toast(id);
    }

    pub(super) fn on_remote(&mut self, generation: u64, ev: Event) {
        if generation != self.remote.generation {
            // stale connection: refuse prompts so its task ends
            match ev {
                Event::HostKeyUnknown { reply, .. } => {
                    let _ = reply.send(false);
                }
                Event::Prompt { reply, .. } => {
                    let _ = reply.send(None);
                }
                _ => {}
            }
            return;
        }
        match ev {
            Event::Status(msg) => {
                if let (Some(t), Some(h)) = (self.remote.toast, &self.remote.host) {
                    if let Some(entry) = self.toasts.iter_mut().find(|(i, _)| *i == t) {
                        entry.1.message = format!("Setting up SSH Host {}: {msg}", h.alias).into();
                        self.sync_toasts();
                    }
                } else {
                    self.toast(0, msg);
                }
            }
            Event::HostKeyUnknown { host, fingerprint, reply } => {
                self.hostkey_reply = Some(reply);
                let title = format!("\"{host}\" has fingerprint \"{fingerprint}\".");
                let rows = vec![QRow::text("Continue", QAction::Choice(0)), QRow::text("Cancel", QAction::Choice(1))];
                self.open_quick(QMode::Pick(PickKind::HostKey), "", "Are you sure you want to continue?", &title, rows);
            }
            Event::Prompt { title, password, reply } => {
                let _ = password;
                self.open_quick(QMode::Secret(Some(reply)), "", "Press 'Enter' to confirm or 'Escape' to cancel", &title, Vec::new());
                self.ui().set_quick_password(true);
            }
            Event::Connected { home } => {
                self.remote.connected = true;
                self.remote.home = home.clone();
                if let Some(t) = self.remote.toast.take() {
                    self.remote_toast_done(t);
                }
                self.reload_hosts();
                self.refresh_title();
                self.open_browse(BrowsePurpose::OpenFolder, home);
            }
            Event::Failed(msg) => {
                if let Some(t) = self.remote.toast.take() {
                    self.remote_toast_done(t);
                }
                self.remote.tx = None;
                self.remote.connected = false;
                self.remote.host = None;
                if self.q_open && matches!(self.q_mode, QMode::Secret(_) | QMode::Pick(PickKind::HostKey)) {
                    self.close_quick();
                }
                self.toast(2, msg);
                self.reload_hosts();
                self.refresh_title();
            }
            Event::Closed(reason) => {
                let alias = self.remote.host.as_ref().map(|h| h.alias.clone()).unwrap_or_default();
                self.remote.tx = None;
                self.remote.connected = false;
                self.remote.host = None;
                if matches!(self.ws, Ws::Remote(_)) {
                    self.set_workspace(Ws::None);
                    self.toast(2, format!("Disconnected from SSH host '{alias}': {reason}"));
                }
                self.reload_hosts();
                self.refresh_title();
            }
            Event::Dir { path, result } => self.on_dir(path, result),
            Event::File { path, result } => self.on_remote_file(path, result),
            Event::Written { path, hash, result } => self.on_written(path, hash, result),
            Event::Done { op, path, result } => self.on_done(op, path, result),
            Event::IsDir { path, result } => match result {
                Ok(true) => self.open_remote_folder(path),
                Ok(false) => {
                    self.toast(2, format!("'{path}' is not a folder."));
                }
                Err(e) => {
                    self.toast(2, format!("Unable to open '{path}': {e}"));
                }
            },
            Event::Walked { root, files, truncated } => self.on_walked(root, files, truncated),
            Event::ExecLines { id, lines } => self.on_exec_lines(id, lines),
            Event::ExecDone { id, error } => self.on_exec_done(id, error),
        }
    }

    pub fn host_clicked(&mut self, idx: usize) {
        if let Some(h) = self.hosts.get(idx).cloned() {
            self.connect_host(h);
        }
    }

    pub fn open_ssh_config(&mut self) {
        let Some(p) = ssh_config::config_path() else { return };
        if !p.exists() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&p, b"# Read more about SSH config files: https://linux.die.net/man/5/ssh_config\nHost alias\n    HostName hostname\n    User user\n");
        }
        self.open_local(&p);
        self.ui().invoke_focus_editor();
    }

    pub fn close_remote(&mut self) {
        if self.remote.tx.is_some() {
            self.confirm_then(Then::Disconnect);
        }
    }

    pub fn remote_action(&mut self, a: &str) {
        match a {
            "config" => self.open_ssh_config(),
            "refresh" => self.reload_hosts(),
            _ => {}
        }
    }
}

fn walk_local(root: &Path) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([PathBuf::new()]);
    while let Some(rel) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(root.join(&rel)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = rel.join(&name);
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    if !remote::SKIP_DIRS.contains(&name.as_str()) {
                        queue.push_back(r);
                    }
                }
                Ok(_) => {
                    out.push(r.to_string_lossy().replace('\\', "/"));
                    if out.len() >= 20_000 {
                        return (out, true);
                    }
                }
                Err(_) => {}
            }
        }
    }
    (out, false)
}
