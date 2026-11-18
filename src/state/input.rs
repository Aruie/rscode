//! Quick input, command palette, global keybindings, menus, find widget, side bar.

use super::*;

impl State {
    // ------------------------------------------------------------------ quick input

    pub(super) fn open_quick(&mut self, mode: QMode, text: &str, placeholder: &str, title: &str, rows: Vec<QRow>) {
        let app = self.ui();
        // a pending secret prompt must be answered before it's replaced
        self.cancel_pending_prompt();
        self.q_return_tree = app.get_tree_focused() || (self.q_open && self.q_return_tree);
        self.close_popup();
        self.q_mode = mode;
        self.q_all = rows.clone();
        self.q_rows = rows;
        app.set_quick_password(false);
        app.set_quick_ok(matches!(self.q_mode, QMode::Browse(_)));
        app.set_quick_cursor_pos(-1);
        app.set_quick_text(text.into());
        app.set_quick_placeholder(placeholder.into());
        app.set_quick_title(title.into());
        app.set_quick_index(0);
        self.sync_quick();
        app.set_quick_open(true);
        self.q_open = true;
        if !text.is_empty() {
            self.quick_edited(text);
        }
    }

    fn cancel_pending_prompt(&mut self) {
        if let QMode::Secret(tx) = &mut self.q_mode {
            if let Some(tx) = tx.take() {
                let _ = tx.send(None);
            }
        }
        if let QMode::Pick(PickKind::HostKey) = self.q_mode {
            if let Some(tx) = self.hostkey_reply.take() {
                let _ = tx.send(false);
            }
        }
    }

    fn sync_quick(&self) {
        let items: Vec<QuickItem> = self
            .q_rows
            .iter()
            .map(|r| QuickItem {
                label: r.label.as_str().into(),
                desc: r.desc.as_str().into(),
                detail: SharedString::new(),
                glyph: r.glyph.into(),
                glyph_color: lang::color(r.color),
                keybinding: r.key.into(),
                selectable: !matches!(r.action, QAction::None),
            })
            .collect();
        self.m.quick.set_vec(items);
    }

    pub(super) fn close_quick(&mut self) {
        let app = self.ui();
        self.q_open = false;
        self.q_mode = QMode::Nav;
        app.set_quick_open(false);
        app.set_quick_password(false);
        self.m.quick.set_vec(Vec::new());
        if self.q_return_tree {
            app.invoke_focus_tree();
        } else if self.active.is_some() {
            app.invoke_focus_editor();
        } else {
            app.invoke_focus_root();
        }
        self.q_return_tree = false;
    }

    pub fn quick_cancel(&mut self) {
        self.cancel_pending_prompt();
        self.close_quick();
    }

    pub fn quick_open_files(&mut self, text: &str) {
        self.open_quick(QMode::Nav, text, "Search files by name (append : to go to line or > for commands)", "", Vec::new());
        if text.is_empty() {
            self.quick_edited("");
        } else {
            let app = self.ui();
            app.set_quick_cursor_pos(text.len() as i32);
            app.set_quick_reset_cursor(app.get_quick_reset_cursor() + 1);
        }
    }

    pub fn quick_edited(&mut self, text: &str) {
        match self.q_mode {
            QMode::Nav => {
                self.q_rows = if let Some(cmd) = text.strip_prefix('>') {
                    self.command_rows(cmd.trim())
                } else if let Some(line) = text.strip_prefix(':') {
                    self.goto_rows(line)
                } else {
                    self.file_rows(text.trim())
                };
                let app = self.ui();
                app.set_quick_placeholder(
                    if text.starts_with('>') { "" } else { "Search files by name (append : to go to line or > for commands)" }.into(),
                );
            }
            QMode::Pick(kind) => {
                let mut rows: Vec<(i32, QRow)> = self
                    .q_all
                    .iter()
                    .filter_map(|r| crate::fuzzy::score(text, &format!("{} {}", r.label, r.desc)).map(|s| (s, r.clone())))
                    .collect();
                if !text.is_empty() {
                    rows.sort_by(|a, b| b.0.cmp(&a.0));
                }
                self.q_rows = rows.into_iter().map(|(_, r)| r).collect();
                if kind == PickKind::Hosts && !text.trim().is_empty() {
                    if let Some(h) = HostEntry::ad_hoc(text) {
                        let mut r = QRow::text(format!("➜  {}", h.detail()), QAction::AdHoc(text.trim().to_string()));
                        r.desc = "Connect to this host".into();
                        self.q_rows.insert(0, r);
                    }
                }
            }
            QMode::Browse(_) => {
                let dir = match text.rfind('/') {
                    Some(i) => text[..=i].to_string(),
                    None => String::new(),
                };
                if !dir.is_empty() && dir != self.q_browse_dir {
                    self.q_browse_dir = dir.clone();
                    self.q_browse_entries.clear();
                    let req_dir = if dir.len() > 1 { dir.trim_end_matches('/').to_string() } else { dir.clone() };
                    self.q_browse_dir = req_dir.clone();
                    self.send(Req::ReadDir { path: req_dir });
                }
                self.browse_rows();
                return;
            }
            QMode::Input(_) | QMode::Secret(_) => {}
        }
        self.sync_quick();
    }

    fn file_rows(&mut self, query: &str) -> Vec<QRow> {
        let mut rows = Vec::new();
        let ws_open = self.ws_root().is_some();
        if ws_open {
            self.ensure_index();
        }
        if query.is_empty() {
            for &id in &self.mru {
                if let Some(i) = self.tab_index(id) {
                    let loc = &self.tabs[i].doc.loc;
                    let l = self.tabs[i].doc.lang;
                    let rel = self.rel_path(&self.parent_of(&loc.path_string()));
                    rows.push(QRow {
                        label: loc.file_name(),
                        desc: rel,
                        glyph: l.glyph,
                        color: l.color,
                        key: "",
                        action: QAction::Tab(id),
                    });
                }
            }
            if rows.is_empty() && !ws_open {
                rows.push(QRow::text("No editors are open. Open a folder to search for files.", QAction::None));
            }
            return rows;
        }
        if let Some(files) = &self.file_index {
            let mut scored: Vec<(i32, &String)> = files.iter().filter_map(|f| crate::fuzzy::score_path(query, f).map(|s| (s, f))).collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            let root = self.ws_root().unwrap_or_default();
            for (_, f) in scored.into_iter().take(60) {
                let (dir, name) = f.rsplit_once('/').unwrap_or(("", f.as_str()));
                let l = lang::detect(name);
                rows.push(QRow {
                    label: name.to_string(),
                    desc: dir.to_string(),
                    glyph: l.glyph,
                    color: l.color,
                    key: "",
                    action: QAction::Open(self.join(&root, f)),
                });
            }
            if rows.is_empty() {
                rows.push(QRow::text("No matching results", QAction::None));
            }
        } else if self.index_loading {
            rows.push(QRow::text("Searching...", QAction::None));
        } else {
            rows.push(QRow::text("No matching results", QAction::None));
        }
        rows
    }

    fn command_rows(&self, query: &str) -> Vec<QRow> {
        let mut scored: Vec<(i32, &commands::Cmd)> = COMMANDS
            .iter()
            .filter_map(|c| crate::fuzzy::score(query, c.title).map(|s| (s, c)))
            .collect();
        if !query.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0));
        }
        let mut rows: Vec<QRow> = scored
            .into_iter()
            .map(|(_, c)| QRow { label: c.title.into(), desc: String::new(), glyph: "", color: 0, key: c.key, action: QAction::Cmd(c.id) })
            .collect();
        if rows.is_empty() {
            rows.push(QRow::text("No matching commands", QAction::None));
        }
        rows
    }

    fn goto_rows(&self, spec: &str) -> Vec<QRow> {
        let Some(t) = self.active_tab() else {
            return vec![QRow::text("Open a text editor first to go to a line.", QAction::None)];
        };
        let n = t.doc.text.bytes().filter(|&b| b == b'\n').count() + 1;
        let parts: Vec<&str> = spec.trim().split([':', ',']).collect();
        let line = parts.first().and_then(|s| s.trim().parse::<usize>().ok());
        let col = parts.get(1).and_then(|s| s.trim().parse::<usize>().ok());
        match line {
            Some(l) if l >= 1 => {
                let l = l.min(n);
                let label = match col {
                    Some(c) => format!("Go to line {l}, character {c}."),
                    None => format!("Go to line {l}."),
                };
                vec![QRow::text(label, QAction::Choice(0))]
            }
            _ => {
                let (l, c) = ops::line_col(&t.doc.text, t.doc.sel.cursor.min(t.doc.text.len()), t.doc.tab_size);
                vec![QRow::text(format!("Current Line: {l}, Character: {c}. Type a line number between 1 and {n} to navigate to."), QAction::None)]
            }
        }
    }

    pub(super) fn open_browse(&mut self, purpose: BrowsePurpose, start: String) {
        let text = match purpose {
            BrowsePurpose::SaveAs(_) => start,
            _ => {
                if start.ends_with('/') { start } else { format!("{start}/") }
            }
        };
        let title = match purpose {
            BrowsePurpose::OpenFolder => "Open Folder",
            BrowsePurpose::OpenFile => "Open File",
            BrowsePurpose::SaveAs(_) => "Save As",
        };
        self.q_browse_dir = String::new();
        self.q_browse_entries.clear();
        self.open_quick(QMode::Browse(purpose), &text, "", title, Vec::new());
        let app = self.ui();
        app.set_quick_cursor_pos(text.len() as i32);
        app.set_quick_reset_cursor(app.get_quick_reset_cursor() + 1);
    }

    pub(super) fn browse_rows(&mut self) {
        let QMode::Browse(purpose) = self.q_mode else { return };
        let text = self.ui().get_quick_text().to_string();
        let frag = text.rsplit('/').next().unwrap_or("").to_lowercase();
        // Like VS Code's simple file dialog: OK is a button next to the input, not a list row.
        let mut rows = Vec::new();
        if self.q_browse_dir.len() > 1 && frag.is_empty() {
            rows.push(QRow::text("..", QAction::BrowseInto("..".into(), true)));
        }
        for (name, is_dir) in &self.q_browse_entries {
            if !name.to_lowercase().starts_with(&frag) {
                continue;
            }
            if !frag.starts_with('.') && name.starts_with('.') && frag.is_empty() {
                // VS Code hides dotfiles until you type a dot
                continue;
            }
            if purpose == BrowsePurpose::OpenFolder && !is_dir {
                continue;
            }
            let l = lang::detect(name);
            rows.push(QRow {
                label: if *is_dir { format!("{name}/") } else { name.clone() },
                desc: String::new(),
                glyph: if *is_dir { "" } else { l.glyph },
                color: l.color,
                key: "",
                action: QAction::BrowseInto(name.clone(), *is_dir),
            });
        }
        self.q_rows = rows;
        self.sync_quick();
        // typing a fragment selects the first match; otherwise nothing is selected and Enter accepts the typed path
        self.ui().set_quick_index(if !frag.is_empty() && !self.q_rows.is_empty() { 0 } else { -1 });
    }

    pub fn quick_accept(&mut self, idx: i32) {
        let app = self.ui();
        let text = app.get_quick_text().to_string();
        let row = usize::try_from(idx).ok().and_then(|i| self.q_rows.get(i)).cloned();
        let mode = std::mem::replace(&mut self.q_mode, QMode::Nav);
        match mode {
            QMode::Secret(mut tx) => {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(Some(text));
                }
                self.close_quick();
            }
            QMode::Input(kind) => {
                self.close_quick();
                match kind {
                    InputKind::NewFile(dir) => self.create_item(dir, &text, false),
                    InputKind::NewFolder(dir) => self.create_item(dir, &text, true),
                    InputKind::Rename(from, _) => self.rename_item(from, &text),
                }
            }
            QMode::Browse(purpose) => {
                let action = row.map(|r| r.action).unwrap_or(QAction::BrowseOk);
                match action {
                    QAction::BrowseInto(name, is_dir) => {
                        let base = if self.q_browse_dir.is_empty() { "/".to_string() } else { self.q_browse_dir.clone() };
                        let new = if name == ".." { remote::parent(&base) } else { remote::join(&base, &name) };
                        if is_dir {
                            self.q_mode = QMode::Browse(purpose);
                            let t = if new.ends_with('/') { new } else { format!("{new}/") };
                            app.set_quick_text(t.as_str().into());
                            app.set_quick_cursor_pos(t.len() as i32);
                            app.set_quick_reset_cursor(app.get_quick_reset_cursor() + 1);
                            self.quick_edited(&t);
                        } else {
                            match purpose {
                                BrowsePurpose::SaveAs(id) => {
                                    self.close_quick();
                                    self.finish_save_as(id, Where::Remote(new));
                                }
                                _ => {
                                    self.close_quick();
                                    self.open_remote(new);
                                }
                            }
                        }
                    }
                    _ => {
                        if purpose != BrowsePurpose::OpenFolder && text.ends_with('/') {
                            // a folder path is not a file: stay in the dialog
                            self.q_mode = QMode::Browse(purpose);
                            return;
                        }
                        self.close_quick();
                        let path = if text.len() > 1 { text.trim_end_matches('/').to_string() } else { text.clone() };
                        match purpose {
                            BrowsePurpose::OpenFolder => {
                                self.send(Req::IsDir { path });
                            }
                            BrowsePurpose::OpenFile => self.open_remote(path),
                            BrowsePurpose::SaveAs(id) => self.finish_save_as(id, Where::Remote(path)),
                        }
                    }
                }
            }
            QMode::Pick(kind) => {
                let action = row.map(|r| r.action).unwrap_or(QAction::None);
                if kind == PickKind::HostKey {
                    let yes = matches!(action, QAction::Choice(0));
                    if let Some(tx) = self.hostkey_reply.take() {
                        let _ = tx.send(yes);
                    }
                    self.close_quick();
                    return;
                }
                self.close_quick();
                match (kind, action) {
                    (PickKind::RemoteMenu, QAction::Cmd(c)) => self.run_command(c),
                    (PickKind::Hosts, QAction::Host(i)) => self.host_clicked(i),
                    (PickKind::Hosts, QAction::AdHoc(spec)) => {
                        if let Some(h) = HostEntry::ad_hoc(&spec) {
                            self.connect_host(h);
                        }
                    }
                    (PickKind::Hosts, QAction::Cmd(c)) => self.run_command(c),
                    (PickKind::Eol, QAction::Choice(c)) => {
                        if let Some(i) = self.active {
                            self.tabs[i].doc.crlf = c == 1;
                            // EOL change marks the file dirty in VS Code; force by breaking the saved hash
                            self.tabs[i].doc.mark_saved(0);
                            self.refresh_tabs();
                            self.refresh_status();
                            self.refresh_title();
                        }
                    }
                    (PickKind::Lang, QAction::Choice(c)) => {
                        if let (Some(i), Some(l)) = (self.active, lang::all().nth(c)) {
                            self.tabs[i].doc.lang = l;
                            let reg = crate::syntax::registry();
                            self.tabs[i].doc.grammar = reg.by_lang_name(l.name);
                            self.tabs[i].doc.spans.clear();
                            self.schedule_highlight();
                            self.refresh_tabs();
                            self.refresh_status();
                        }
                    }
                    (PickKind::Indent, QAction::Choice(c)) => {
                        if let Some(i) = self.active {
                            self.tabs[i].doc.use_tabs = c == 1;
                            self.refresh_status();
                        }
                    }
                    _ => {}
                }
            }
            QMode::Nav => {
                let action = row.map(|r| r.action).unwrap_or(QAction::None);
                match action {
                    QAction::None => {
                        self.q_mode = QMode::Nav;
                        return;
                    }
                    QAction::Cmd(c) => {
                        self.close_quick();
                        self.run_command(c);
                    }
                    QAction::Open(p) => {
                        self.close_quick();
                        self.open_path(&p);
                        app.invoke_focus_editor();
                    }
                    QAction::Tab(id) => {
                        self.close_quick();
                        if let Some(i) = self.tab_index(id) {
                            self.activate(i);
                            app.invoke_focus_editor();
                        }
                    }
                    QAction::Choice(_) => {
                        self.close_quick();
                        self.goto_line(text.trim_start_matches(':'));
                    }
                    _ => self.close_quick(),
                }
            }
        }
    }

    fn goto_line(&mut self, spec: &str) {
        let Some(i) = self.active else { return };
        let parts: Vec<&str> = spec.trim().split([':', ',']).collect();
        let line = parts.first().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1).max(1);
        let col = parts.get(1).and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1).max(1);
        let t = &self.tabs[i].doc.text;
        let p = ops::offset_of(t, line, col);
        let l0 = ops::line_index(t, p);
        self.center_line(l0);
        self.set_sel(Sel::caret(p));
        self.ui().invoke_focus_editor();
    }

    fn pick(&mut self, kind: PickKind, placeholder: &str, rows: Vec<QRow>) {
        self.open_quick(QMode::Pick(kind), "", placeholder, "", rows);
    }

    fn input(&mut self, kind: InputKind, text: &str, placeholder: &str, title: &str) {
        self.open_quick(QMode::Input(kind), text, placeholder, title, Vec::new());
    }

    // ------------------------------------------------------------------ commands

    pub fn run_command(&mut self, id: &str) {
        let app = self.ui();
        match id {
            "file.new" => self.new_untitled(),
            "file.open" => self.open_file_dialog(),
            "file.openFolder" => self.open_folder(),
            "file.save" => self.save_active(),
            "file.saveAs" => {
                if let Some(t) = self.active_tab() {
                    let id = t.id;
                    self.save_as(id);
                }
            }
            "file.saveAll" => self.save_all(),
            "file.revert" => self.revert(),
            "file.closeFolder" => {
                if self.ws != Ws::None {
                    self.confirm_then(Then::CloseFolder);
                }
            }
            "editor.close" => {
                if let Some(i) = self.active {
                    self.request_close_tab(i);
                }
            }
            "editor.closeAll" => self.confirm_then(Then::CloseEditors),
            "editor.reopen" => self.reopen_closed(),
            "editor.next" => self.cycle_tab(1),
            "editor.prev" => self.cycle_tab(-1),
            "view.sidebar" => {
                self.sidebar = !self.sidebar;
                self.sync_sidebar();
            }
            "view.explorer" => {
                self.show_view("explorer");
                if self.ws != Ws::None {
                    app.invoke_focus_tree();
                }
            }
            "view.search" => self.open_search_view(),
            "view.scm" => self.show_view("scm"),
            "view.run" => self.show_view("run"),
            "view.extensions" => self.show_view("extensions"),
            "view.remote" => self.show_view("remote"),
            "view.zoomIn" => self.zoom(1.0),
            "view.zoomOut" => self.zoom(-1.0),
            "view.zoomReset" => self.zoom(0.0),
            "quickOpen" => self.quick_open_files(""),
            "palette" => self.quick_open_files(">"),
            "gotoLine" => self.quick_open_files(":"),
            "find" => self.open_find(false),
            "replace" => self.open_find(true),
            "editor.undo" => self.undo(),
            "editor.redo" => self.redo(),
            "editor.cut" | "editor.copy" | "editor.paste" | "editor.selectAll" => {
                let w = self.app.clone();
                let id = id.to_string();
                app.invoke_focus_editor();
                later(move || {
                    if let Some(a) = w.upgrade() {
                        match id.as_str() {
                            "editor.cut" => a.invoke_editor_cut(),
                            "editor.copy" => a.invoke_editor_copy(),
                            "editor.paste" => a.invoke_editor_paste(),
                            _ => {
                                let len = a.get_editor_text().len() as i32;
                                a.invoke_set_selection(0, len);
                            }
                        }
                    }
                });
            }
            "editor.comment" | "editor.moveUp" | "editor.moveDown" | "editor.copyUp" | "editor.copyDown" | "editor.deleteLine"
            | "editor.indent" | "editor.outdent" | "editor.insertBelow" | "editor.insertAbove" | "editor.selectLine" | "editor.nextMatch" => {
                let (key, ctrl, shift, alt): (SharedString, bool, bool, bool) = match id {
                    "editor.comment" => ("/".into(), true, false, false),
                    "editor.moveUp" => (Key::UpArrow.into(), false, false, true),
                    "editor.moveDown" => (Key::DownArrow.into(), false, false, true),
                    "editor.copyUp" => (Key::UpArrow.into(), false, true, true),
                    "editor.copyDown" => (Key::DownArrow.into(), false, true, true),
                    "editor.deleteLine" => ("k".into(), true, true, false),
                    "editor.indent" => ("]".into(), true, false, false),
                    "editor.outdent" => ("[".into(), true, false, false),
                    "editor.insertBelow" => (Key::Return.into(), true, false, false),
                    "editor.insertAbove" => (Key::Return.into(), true, true, false),
                    "editor.selectLine" => ("l".into(), true, false, false),
                    _ => ("d".into(), true, false, false),
                };
                self.editor_key(&key, ctrl, shift, alt);
                app.invoke_focus_editor();
            }
            "editor.eol" => {
                if self.active.is_some() {
                    self.pick(PickKind::Eol, "Select End of Line Sequence", vec![QRow::text("LF", QAction::Choice(0)), QRow::text("CRLF", QAction::Choice(1))]);
                }
            }
            "editor.useSpaces" | "editor.useTabs" => {
                if let Some(i) = self.active {
                    self.tabs[i].doc.use_tabs = id == "editor.useTabs";
                    self.refresh_status();
                }
            }
            "editor.indentPick" => {
                if self.active.is_some() {
                    self.pick(
                        PickKind::Indent,
                        "Select Action",
                        vec![QRow::text("Indent Using Spaces", QAction::Choice(0)), QRow::text("Indent Using Tabs", QAction::Choice(1))],
                    );
                }
            }
            "editor.lang" => {
                if self.active.is_some() {
                    let rows = lang::all()
                        .enumerate()
                        .map(|(i, l)| QRow { label: l.name.into(), desc: String::new(), glyph: l.glyph, color: l.color, key: "", action: QAction::Choice(i) })
                        .collect();
                    self.pick(PickKind::Lang, "Select Language Mode", rows);
                }
            }
            "remote.connect" => {
                self.reload_hosts();
                let mut rows: Vec<QRow> = self
                    .hosts
                    .iter()
                    .enumerate()
                    .map(|(i, h)| QRow { label: h.alias.clone(), desc: h.detail(), glyph: "", color: 0, key: "", action: QAction::Host(i) })
                    .collect();
                rows.push(QRow::text("+ Add New SSH Host...", QAction::Cmd("remote.openConfig")));
                rows.push(QRow::text("Configure SSH Hosts...", QAction::Cmd("remote.openConfig")));
                self.pick(PickKind::Hosts, "Select configured SSH host or enter user@host", rows);
            }
            "remote.openConfig" => self.open_ssh_config(),
            "remote.close" => self.close_remote(),
            "remote.menu" => {
                let rows = if self.remote.connected {
                    vec![
                        QRow { label: "Open Folder...".into(), desc: "Remote-SSH".into(), glyph: "", color: 0, key: "", action: QAction::Cmd("file.openFolder") },
                        QRow { label: "Close Remote Connection".into(), desc: String::new(), glyph: "", color: 0, key: "", action: QAction::Cmd("remote.close") },
                    ]
                } else {
                    vec![
                        QRow { label: "Connect to Host...".into(), desc: "Remote-SSH".into(), glyph: "", color: 0, key: "", action: QAction::Cmd("remote.connect") },
                        QRow {
                            label: "Open SSH Configuration File...".into(),
                            desc: "Remote-SSH".into(),
                            glyph: "",
                            color: 0,
                            key: "",
                            action: QAction::Cmd("remote.openConfig"),
                        },
                    ]
                };
                self.pick(PickKind::RemoteMenu, "Select an option to open a Remote Window", rows);
            }
            "app.quit" => self.request_quit(),
            // explorer context menu
            "tree.newFile" | "tree.newFolder" => {
                if let Some(dir) = self.target_dir() {
                    let kind = if id == "tree.newFile" { InputKind::NewFile(dir.clone()) } else { InputKind::NewFolder(dir.clone()) };
                    let rel = self.rel_path(&dir);
                    let title = if rel.is_empty() { self.ws_name() } else { rel };
                    let ph = if id == "tree.newFile" { "File name" } else { "Folder name" };
                    self.input(kind, "", ph, &format!("New in {title}"));
                }
            }
            "tree.rename" => {
                if let Some(i) = self.tree.selected {
                    let n = &self.tree.nodes[i];
                    let (path, name, is_dir) = (n.path.clone(), n.name.clone(), n.is_dir);
                    self.input(InputKind::Rename(path, is_dir), &name, "New name", "Rename");
                }
            }
            "tree.delete" => {
                if let Some(i) = self.tree.selected {
                    let n = &self.tree.nodes[i];
                    let (path, name, is_dir) = (n.path.clone(), n.name.clone(), n.is_dir);
                    let what = if is_dir { format!("Are you sure you want to permanently delete '{name}' and its contents?") } else { format!("Are you sure you want to permanently delete '{name}'?") };
                    self.show_dialog(Dialog::Delete(path, is_dir), &what, "This action is irreversible!", &["Delete", "Cancel"]);
                }
            }
            "tree.refresh" => {
                let dirs: Vec<String> = self.tree.nodes.iter().filter(|n| n.is_dir && n.loaded && (n.expanded || n.parent.is_none()) && !n.path.is_empty()).map(|n| n.path.clone()).collect();
                for d in dirs {
                    self.refresh_dir(&d);
                }
                self.file_index = None;
            }
            "tree.revealSsh" => {}
            _ => {}
        }
    }

    // ------------------------------------------------------------------ global keybindings

    pub fn global_key(&mut self, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        let text: &str = &shortcut_key(text, ctrl, alt);
        let lower = text.to_lowercase();
        let k = lower.as_str();
        let app = self.ui();
        if self.chord {
            if is_key(text, Key::Control) || is_key(text, Key::Shift) || is_key(text, Key::Alt) {
                return false;
            }
            self.chord = false;
            app.set_status_message("".into());
            // the second key of a chord may come without Ctrl ("Ctrl+K F"), so map jamo here too
            let text: &str = &shortcut_key(text, true, false);
            let lower = text.to_lowercase();
            let k = lower.as_str();
            let cmd = match (ctrl, shift, alt, k) {
                (true, false, false, "o") => Some("file.openFolder"),
                (false, false, false, "f") => Some("file.closeFolder"),
                (false, false, false, "s") => Some("file.saveAll"),
                (true, false, false, "w") => Some("editor.closeAll"),
                (false, false, false, "m") => Some("editor.lang"),
                (true, false, false, "c") => Some("editor.comment"),
                _ => None,
            };
            match cmd {
                Some(c) => self.run_command(c),
                None => {
                    let shown = if text.chars().count() == 1 && !text.chars().next().unwrap().is_control() { text.to_uppercase() } else { "key".into() };
                    let msg = format!("The key combination (Ctrl+K, {}{shown}) is not a command.", if ctrl { "Ctrl+" } else { "" });
                    app.set_status_message(msg.into());
                    slint::Timer::single_shot(Duration::from_secs(3), move || {
                        with_state(|s| {
                            if !s.chord {
                                s.ui().set_status_message("".into());
                            }
                        });
                    });
                }
            }
            return true;
        }
        if self.q_open {
            // inside the quick input only its own shortcuts apply
            return match (ctrl, shift, k) {
                (true, false, "p") | (true, false, "e") => {
                    self.quick_open_files("");
                    true
                }
                (true, true, "p") => {
                    self.quick_open_files(">");
                    true
                }
                _ => false,
            };
        }
        if is_key(text, Key::F1) {
            self.run_command("palette");
            return true;
        }
        if is_key(text, Key::F3) {
            self.find_step(shift);
            return true;
        }
        if is_key(text, Key::F4) && ctrl {
            self.run_command("editor.close");
            return true;
        }
        if is_key(text, Key::Escape) {
            if self.popup.is_some() {
                self.close_popup();
                return true;
            }
            if self.find.open {
                self.find_close();
                return true;
            }
            return false;
        }
        if ctrl && is_key(text, Key::Tab) {
            self.mru_switch(shift);
            return true;
        }
        if ctrl && is_key(text, Key::Backtab) {
            self.mru_switch(true);
            return true;
        }
        if ctrl && !alt && is_key(text, Key::PageDown) {
            self.run_command("editor.next");
            return true;
        }
        if ctrl && !alt && is_key(text, Key::PageUp) {
            self.run_command("editor.prev");
            return true;
        }
        if !ctrl {
            return false;
        }
        let cmd = match (shift, alt, k) {
            (false, false, "k") => {
                self.chord = true;
                app.set_status_message("(Ctrl+K) was pressed. Waiting for second key of chord...".into());
                return true;
            }
            (false, false, "s") => "file.save",
            (true, false, "s") => "file.saveAs",
            (false, false, "n") => "file.new",
            (false, false, "o") => "file.open",
            (false, false, "w") => "editor.close",
            (true, false, "t") => "editor.reopen",
            (false, false, "b") => "view.sidebar",
            (true, false, "e") => "view.explorer",
            (true, false, "f") => "view.search",
            (true, false, "g") => "view.scm",
            (true, false, "d") => "view.run",
            (true, false, "x") => "view.extensions",
            (false, false, "p") | (false, false, "e") => "quickOpen",
            (true, false, "p") => "palette",
            (false, false, "g") => "gotoLine",
            (false, false, "f") => "find",
            (false, false, "h") => "replace",
            (false, true, "o") => "remote.menu",
            (_, false, "=") | (_, false, "+") => "view.zoomIn",
            (_, false, "-") => "view.zoomOut",
            (false, false, "0") => {
                if self.sidebar {
                    app.invoke_focus_tree();
                } else {
                    self.sidebar = true;
                    self.sync_sidebar();
                }
                return true;
            }
            (false, false, "1") => {
                if self.active.is_some() {
                    app.invoke_focus_editor();
                }
                return true;
            }
            _ => return false,
        };
        self.run_command(cmd);
        true
    }

    // ------------------------------------------------------------------ side bar / views

    pub(super) fn sync_sidebar(&self) {
        let app = self.ui();
        app.set_sidebar_visible(self.sidebar);
        app.set_active_view(self.view.into());
    }

    pub(super) fn show_view(&mut self, v: &'static str) {
        self.view = v;
        self.sidebar = true;
        self.sync_sidebar();
    }

    pub fn activity(&mut self, v: &str) {
        let v: &'static str = match v {
            "explorer" => "explorer",
            "search" => "search",
            "scm" => "scm",
            "run" => "run",
            "extensions" => "extensions",
            "remote" => "remote",
            "account" | "manage" => {
                self.toast(0, "Accounts and settings are not available yet.".into());
                return;
            }
            _ => return,
        };
        if self.sidebar && self.view == v {
            self.sidebar = false;
        } else {
            self.view = v;
            self.sidebar = true;
        }
        self.sync_sidebar();
    }

    fn zoom(&mut self, d: f32) {
        self.font_size = if d == 0.0 { 14.0 } else { (self.font_size + d).clamp(6.0, 40.0) };
        self.ui().set_editor_font_size(self.font_size);
    }

    pub fn status_clicked(&mut self, which: &str) {
        match which {
            "remote" => self.run_command("remote.menu"),
            "eol" => self.run_command("editor.eol"),
            "indent" => self.run_command("editor.indentPick"),
            "lang" => self.run_command("editor.lang"),
            "lncol" => self.run_command("gotoLine"),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ popup menus

    fn show_popup(&mut self, p: Popup, x: f32, y: f32, from_menubar: bool) {
        let ids: &Vec<&'static str> = match &p {
            Popup::Menu(v) | Popup::Context(v) => v,
        };
        let items: Vec<MenuItemData> = ids
            .iter()
            .map(|id| {
                if *id == "-" {
                    return MenuItemData { label: "".into(), keybinding: "".into(), separator: true, enabled: false };
                }
                let (label, enabled) = label_for(id);
                let key = match *id {
                    "tree.rename" => "F2",
                    "tree.delete" => "Delete",
                    _ if enabled => commands::key_of(id),
                    _ => commands::disabled_key(id),
                };
                MenuItemData { label: label.into(), keybinding: key.into(), separator: false, enabled: enabled && self.command_enabled(id) }
            })
            .collect();
        self.m.popup.set_vec(items);
        let app = self.ui();
        app.set_popup_x(x);
        app.set_popup_y(y);
        app.set_popup_from_menubar(from_menubar);
        app.set_popup_open(true);
        self.popup = Some(p);
    }

    fn command_enabled(&self, id: &str) -> bool {
        let editor = self.active_tab().is_some_and(|t| t.doc.message.is_empty());
        match id {
            "file.save" | "file.saveAs" | "file.revert" | "editor.close" => self.active.is_some(),
            "file.closeFolder" => self.ws != Ws::None,
            "remote.close" => self.remote.tx.is_some(),
            "tree.rename" | "tree.delete" => self.tree.selected.is_some(),
            s if s.starts_with("editor.") && s != "editor.next" && s != "editor.prev" => editor,
            "find" | "replace" | "gotoLine" => editor,
            _ => true,
        }
    }

    pub fn close_popup(&mut self) {
        if self.popup.take().is_some() {
            let app = self.ui();
            app.set_popup_open(false);
            app.set_menu_index(-1);
            if self.q_open {
                return;
            }
            if self.active.is_some() && !app.get_tree_focused() {
                app.invoke_focus_editor();
            }
        }
    }

    pub fn menu_open(&mut self, idx: usize, x: f32) {
        let mut ids = Vec::new();
        for e in commands::menu(idx) {
            match e {
                Entry::Item(id, _) | Entry::Disabled(id, _) => ids.push(*id),
                Entry::Sep => ids.push("-"),
            }
        }
        self.show_popup(Popup::Menu(ids), x, 34.0, true);
        self.ui().set_menu_index(idx as i32);
    }

    pub fn tree_context(&mut self, row: i32, x: f32, y: f32) {
        self.tree.selected = if row >= 0 { self.tree.visible.get(row as usize).copied() } else { None };
        self.refresh_tree();
        let ids = vec!["tree.newFile", "tree.newFolder", "-", "tree.rename", "tree.delete", "-", "tree.refresh"];
        self.show_popup(Popup::Context(ids), x, y, false);
    }

    pub fn popup_activated(&mut self, idx: usize) {
        let id = match &self.popup {
            Some(Popup::Menu(v)) | Some(Popup::Context(v)) => v.get(idx).copied(),
            None => None,
        };
        let was_tree = matches!(self.popup, Some(Popup::Context(_)));
        self.close_popup();
        if let Some(id) = id.filter(|i| *i != "-") {
            if was_tree {
                self.ui().invoke_focus_tree();
            }
            if self.command_enabled(id) {
                self.run_command(id);
            }
        }
    }

    // ------------------------------------------------------------------ find / replace

    pub fn open_find(&mut self, replace: bool) {
        let Some(t) = self.active_tab() else { return };
        if !t.doc.message.is_empty() {
            return;
        }
        let sel = self.widget_sel();
        let txt = &t.doc.text;
        if !sel.is_empty() {
            let s = &txt[sel.start()..sel.end()];
            if !s.contains('\n') {
                self.find.text = s.to_string();
            }
        } else {
            let (a, b) = ops::word_at(txt, sel.cursor);
            if a < b {
                self.find.text = txt[a..b].to_string();
            }
        }
        self.find.open = true;
        self.find.replace = replace;
        let app = self.ui();
        app.set_find_text(self.find.text.as_str().into());
        app.set_find_open(true);
        app.set_replace_open(replace);
        self.find_count();
        if replace && !self.find.text.is_empty() {
            app.invoke_focus_replace();
        } else {
            app.invoke_focus_find();
        }
    }

    pub fn find_close(&mut self) {
        self.find.open = false;
        let app = self.ui();
        app.set_find_open(false);
        if self.active.is_some() {
            app.invoke_focus_editor();
        }
    }

    pub fn find_toggle_replace(&mut self) {
        self.find.replace = !self.find.replace;
        self.ui().set_replace_open(self.find.replace);
    }

    pub fn find_toggle(&mut self, what: &str) {
        match what {
            "case" => self.find.case = !self.find.case,
            _ => self.find.word = !self.find.word,
        }
        let app = self.ui();
        app.set_find_case(self.find.case);
        app.set_find_word(self.find.word);
        self.find_count();
    }

    fn matches(&self) -> Vec<(usize, usize)> {
        match self.active_tab() {
            Some(t) => ops::find_all(&t.doc.text, &self.find.text, self.find.case, self.find.word, 20_000),
            None => Vec::new(),
        }
    }

    /// Update "x of y" without moving.
    pub(super) fn find_count(&self) {
        let app = self.ui();
        if self.find.text.is_empty() {
            app.set_find_info("No results".into());
            return;
        }
        let m = self.matches();
        let sel = self.active_tab().map(|t| t.doc.sel).unwrap_or_default();
        let info = if m.is_empty() {
            "No results".to_string()
        } else {
            match m.iter().position(|&(s, e)| s == sel.start() && e == sel.end()) {
                Some(i) => format!("{} of {}", i + 1, m.len()),
                None => format!("? of {}", m.len()),
            }
        };
        app.set_find_info(info.into());
    }

    pub fn find_edited(&mut self, text: &str) {
        self.find.text = text.to_string();
        // incremental: first match at or after the current selection start
        let m = self.matches();
        let start = self.active_tab().map(|t| t.doc.sel.start()).unwrap_or(0);
        if let Some(&(s, e)) = m.iter().find(|x| x.0 >= start).or(m.first()) {
            self.select_match(s, e);
        }
        self.find_count();
    }

    fn select_match(&mut self, s: usize, e: usize) {
        let Some(i) = self.active else { return };
        let line = ops::line_index(&self.tabs[i].doc.text, s);
        let app = self.ui();
        let lh = app.get_line_h();
        let y = -app.get_editor_scroll_y();
        let vis = app.get_editor_visible_height();
        let ly = line as f32 * lh;
        if ly < y || ly + lh > y + vis {
            self.center_line(line);
        }
        self.set_sel(Sel::new(s, e));
    }

    pub fn find_step(&mut self, backward: bool) {
        if !self.find.open {
            if self.find.text.is_empty() {
                self.open_find(false);
                return;
            }
        }
        let m = self.matches();
        if m.is_empty() {
            self.find_count();
            return;
        }
        let sel = self.widget_sel();
        let pick = if backward {
            m.iter().rev().find(|x| x.0 < sel.start()).or(m.last())
        } else {
            m.iter().find(|x| x.0 > sel.start()).or(m.first())
        };
        if let Some(&(s, e)) = pick {
            self.select_match(s, e);
        }
        self.find_count();
    }

    pub fn replace_one(&mut self) {
        let app = self.ui();
        self.find.replace_text = app.get_replace_text().to_string();
        let sel = self.widget_sel();
        let m = self.matches();
        if m.iter().any(|&(s, e)| s == sel.start() && e == sel.end()) {
            let rep = self.find.replace_text.clone();
            let caret = sel.start() + rep.len();
            self.apply_edit(Edit { start: sel.start(), end: sel.end(), insert: rep, sel: Sel::caret(caret) });
            let m = self.matches();
            if let Some(&(s, e)) = m.iter().find(|x| x.0 >= caret).or(m.first()) {
                self.select_match(s, e);
            }
            self.find_count();
        } else {
            self.find_step(false);
        }
    }

    pub fn replace_all(&mut self) {
        let app = self.ui();
        self.find.replace_text = app.get_replace_text().to_string();
        let m = self.matches();
        let Some(i) = self.active else { return };
        if m.is_empty() {
            return;
        }
        let t = &self.tabs[i].doc.text;
        let mut out = String::with_capacity(t.len());
        let mut last = 0;
        for &(s, e) in &m {
            out.push_str(&t[last..s]);
            out.push_str(&self.find.replace_text);
            last = e;
        }
        out.push_str(&t[last..]);
        let len = t.len();
        let caret = self.widget_sel().cursor.min(out.len());
        let caret = ops::floor_char(&out, caret);
        self.apply_edit(Edit { start: 0, end: len, insert: out, sel: Sel::caret(caret) });
        app.set_find_info(format!("Replaced {}", m.len()).into());
    }
}

fn label_for(id: &str) -> (&'static str, bool) {
    for idx in 0..8 {
        for e in commands::menu(idx) {
            match e {
                Entry::Item(i, l) if *i == id => return (l, true),
                Entry::Disabled(i, l) if *i == id => return (l, false),
                _ => {}
            }
        }
    }
    match id {
        "tree.newFile" => ("New File...", true),
        "tree.newFolder" => ("New Folder...", true),
        "tree.rename" => ("Rename...", true),
        "tree.delete" => ("Delete", true),
        "tree.refresh" => ("Refresh", true),
        _ => ("", false),
    }
}
