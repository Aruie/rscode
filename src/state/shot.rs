//! Scenes for the development screenshot tool.

use super::*;

impl State {
    pub fn shot_scene(&mut self, scene: &str) {
        if scene == "welcome" {
            return;
        }
        if scene == "remote" {
            self.activity("remote");
            self.run_command("remote.connect");
            return;
        }
        let root = std::env::current_dir().unwrap();
        self.open_local_folder_now(root.clone());
        if let Some(row) = self.tree.visible.iter().position(|&i| self.tree.nodes[i].name == "src") {
            self.tree_clicked(row);
        }
        self.open_local(&root.join("Cargo.toml"));
        self.open_local(&root.join("src/text_ops.rs"));
        let t = self.tabs[self.active.unwrap()].doc.text.clone();
        let p = t.find("pub fn line_start").unwrap_or(0);
        self.set_sel(Sel::caret(p));
        match scene {
            "palette" => self.quick_open_files(">"),
            "quickopen" => {
                self.file_index = Some(vec!["src/main.rs".into(), "src/state.rs".into(), "ui/app.slint".into(), "Cargo.toml".into()]);
                self.quick_open_files("");
                self.ui().set_quick_text("ma".into());
                self.quick_edited("ma");
            }
            "menu" => self.menu_open(0, 58.0),
            "find" => {
                self.open_find(true);
                self.find_edited("line");
            }
            "dialog" => {
                let i = self.active.unwrap();
                let mut d = self.tabs[i].doc.text.clone();
                d.insert_str(0, "// edit\n");
                self.tabs[i].doc.sync_from_widget(&d, Sel::caret(8));
                self.load_editor();
                self.request_close_tab(i);
            }
            "select" => {
                // selection background comes from the input, glyphs from the overlay: they must line up
                let a = t.find("impl Sel").unwrap_or(0);
                let b = t[a..].find("pub fn start").map(|x| a + x + 12).unwrap_or(a);
                self.ui().invoke_focus_editor();
                self.set_sel(Sel::new(a + 5, b));
                self.center_line(10);
            }
            "context" => {
                self.tree_context(0, 120.0, 140.0);
            }
            _ => {}
        }
    }
}
