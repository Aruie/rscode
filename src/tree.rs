//! Explorer tree: lazily loaded folders, flattened into visible rows.

pub struct Node {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub expanded: bool,
    pub loaded: bool,
    pub loading: bool,
    pub depth: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

#[derive(Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: Option<usize>,
    /// Visible rows (node indices), excluding the root itself.
    pub visible: Vec<usize>,
    pub selected: Option<usize>,
}

pub fn sort_entries(entries: &mut [(String, bool)]) {
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
}

impl Tree {
    pub fn reset(&mut self, name: String, path: String) {
        self.nodes.clear();
        self.visible.clear();
        self.selected = None;
        self.nodes.push(Node {
            name,
            path,
            is_dir: true,
            expanded: true,
            loaded: false,
            loading: true,
            depth: 0,
            parent: None,
            children: Vec::new(),
        });
        self.root = Some(0);
    }

    pub fn clear(&mut self) {
        *self = Tree::default();
    }

    pub fn find(&self, path: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.path == path)
    }

    /// Replace the children of the folder at `path`, keeping expansion state of folders that still exist.
    pub fn set_children(&mut self, path: &str, mut entries: Vec<(String, bool)>, join: impl Fn(&str, &str) -> String) -> Option<usize> {
        let idx = self.find(path)?;
        sort_entries(&mut entries);
        let depth = self.nodes[idx].depth + 1;
        let old = std::mem::take(&mut self.nodes[idx].children);
        let mut children = Vec::with_capacity(entries.len());
        for (name, is_dir) in entries {
            let child_path = join(path, &name);
            if let Some(&o) = old.iter().find(|&&o| self.nodes[o].path == child_path && self.nodes[o].is_dir == is_dir) {
                children.push(o);
                continue;
            }
            self.nodes.push(Node {
                name,
                path: child_path,
                is_dir,
                expanded: false,
                loaded: false,
                loading: false,
                depth,
                parent: Some(idx),
                children: Vec::new(),
            });
            children.push(self.nodes.len() - 1);
        }
        // detach removed nodes so `find` no longer hits them
        for o in old {
            if !children.contains(&o) {
                self.detach(o);
            }
        }
        let n = &mut self.nodes[idx];
        n.children = children;
        n.loaded = true;
        n.loading = false;
        self.rebuild();
        Some(idx)
    }

    fn detach(&mut self, idx: usize) {
        let kids = std::mem::take(&mut self.nodes[idx].children);
        self.nodes[idx].path.clear();
        self.nodes[idx].parent = None;
        if self.selected == Some(idx) {
            self.selected = None;
        }
        for k in kids {
            self.detach(k);
        }
    }

    pub fn rebuild(&mut self) {
        self.visible.clear();
        if let Some(r) = self.root {
            let mut stack: Vec<usize> = self.nodes[r].children.iter().rev().copied().collect();
            while let Some(i) = stack.pop() {
                self.visible.push(i);
                let n = &self.nodes[i];
                if n.is_dir && n.expanded {
                    stack.extend(n.children.iter().rev());
                }
            }
        }
    }

    pub fn collapse_all(&mut self) {
        for n in self.nodes.iter_mut().skip(1) {
            n.expanded = false;
        }
        self.rebuild();
    }

    /// Folder to create new items in: the selected folder, the selected file's folder, or the root.
    pub fn target_dir(&self) -> Option<usize> {
        match self.selected {
            Some(i) if self.nodes[i].is_dir => Some(i),
            Some(i) => self.nodes[i].parent.or(self.root),
            None => self.root,
        }
    }
}
