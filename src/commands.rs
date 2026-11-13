//! Command ids, their palette titles and default (Windows) keybindings, and the menu bar.

pub struct Cmd {
    pub id: &'static str,
    pub title: &'static str,
    pub key: &'static str,
}

macro_rules! c {
    ($id:expr, $title:expr, $key:expr) => {
        Cmd { id: $id, title: $title, key: $key }
    };
}

pub static COMMANDS: &[Cmd] = &[
    c!("file.new", "File: New Untitled Text File", "Ctrl+N"),
    c!("file.open", "File: Open File...", "Ctrl+O"),
    c!("file.openFolder", "File: Open Folder...", "Ctrl+K Ctrl+O"),
    c!("file.save", "File: Save", "Ctrl+S"),
    c!("file.saveAs", "File: Save As...", "Ctrl+Shift+S"),
    c!("file.saveAll", "File: Save All", "Ctrl+K S"),
    c!("file.revert", "File: Revert File", ""),
    c!("file.closeFolder", "Workspaces: Close Workspace", "Ctrl+K F"),
    c!("editor.close", "View: Close Editor", "Ctrl+F4"),
    c!("editor.closeAll", "View: Close All Editors", "Ctrl+K Ctrl+W"),
    c!("editor.reopen", "View: Reopen Closed Editor", "Ctrl+Shift+T"),
    c!("editor.next", "View: Open Next Editor", "Ctrl+PageDown"),
    c!("editor.prev", "View: Open Previous Editor", "Ctrl+PageUp"),
    c!("view.sidebar", "View: Toggle Primary Side Bar Visibility", "Ctrl+B"),
    c!("view.explorer", "View: Show Explorer", "Ctrl+Shift+E"),
    c!("view.search", "View: Show Search", "Ctrl+Shift+F"),
    c!("view.scm", "View: Show Source Control", "Ctrl+Shift+G"),
    c!("view.run", "View: Show Run and Debug", "Ctrl+Shift+D"),
    c!("view.extensions", "View: Show Extensions", "Ctrl+Shift+X"),
    c!("view.remote", "View: Show Remote Explorer", ""),
    c!("view.zoomIn", "View: Zoom In", "Ctrl+="),
    c!("view.zoomOut", "View: Zoom Out", "Ctrl+-"),
    c!("view.zoomReset", "View: Reset Zoom", "Ctrl+NumPad0"),
    c!("quickOpen", "Go to File...", "Ctrl+P"),
    c!("palette", "Show All Commands", "Ctrl+Shift+P"),
    c!("gotoLine", "Go to Line/Column...", "Ctrl+G"),
    c!("find", "Find", "Ctrl+F"),
    c!("replace", "Replace", "Ctrl+H"),
    c!("editor.undo", "Undo", "Ctrl+Z"),
    c!("editor.redo", "Redo", "Ctrl+Y"),
    c!("editor.cut", "Cut", "Ctrl+X"),
    c!("editor.copy", "Copy", "Ctrl+C"),
    c!("editor.paste", "Paste", "Ctrl+V"),
    c!("editor.selectAll", "Select All", "Ctrl+A"),
    c!("editor.comment", "Toggle Line Comment", "Ctrl+/"),
    c!("editor.moveUp", "Move Line Up", "Alt+UpArrow"),
    c!("editor.moveDown", "Move Line Down", "Alt+DownArrow"),
    c!("editor.copyUp", "Copy Line Up", "Shift+Alt+UpArrow"),
    c!("editor.copyDown", "Copy Line Down", "Shift+Alt+DownArrow"),
    c!("editor.deleteLine", "Delete Line", "Ctrl+Shift+K"),
    c!("editor.indent", "Indent Line", "Ctrl+]"),
    c!("editor.outdent", "Outdent Line", "Ctrl+["),
    c!("editor.insertBelow", "Insert Line Below", "Ctrl+Enter"),
    c!("editor.insertAbove", "Insert Line Above", "Ctrl+Shift+Enter"),
    c!("editor.selectLine", "Expand Line Selection", "Ctrl+L"),
    c!("editor.nextMatch", "Add Selection To Next Find Match", "Ctrl+D"),
    c!("editor.eol", "Change End of Line Sequence", ""),
    c!("editor.useSpaces", "Indent Using Spaces", ""),
    c!("editor.useTabs", "Indent Using Tabs", ""),
    c!("editor.lang", "Change Language Mode", "Ctrl+K M"),
    c!("remote.connect", "Remote-SSH: Connect to Host...", ""),
    c!("remote.openConfig", "Remote-SSH: Open SSH Configuration File...", ""),
    c!("remote.close", "Remote: Close Remote Connection", ""),
    c!("remote.menu", "Remote: Show Remote Menu", "Ctrl+Alt+O"),
    c!("app.quit", "Exit", "Alt+F4"),
];

pub fn key_of(id: &str) -> &'static str {
    COMMANDS.iter().find(|c| c.id == id).map(|c| c.key).unwrap_or("")
}

pub enum Entry {
    Item(&'static str, &'static str),
    /// Shown but not available yet.
    Disabled(&'static str, &'static str),
    Sep,
}

use Entry::*;

pub fn menu(index: usize) -> &'static [Entry] {
    static FILE: &[Entry] = &[
        Item("file.new", "New Text File"),
        Sep,
        Item("file.open", "Open File..."),
        Item("file.openFolder", "Open Folder..."),
        Item("remote.connect", "Connect to Host..."),
        Sep,
        Item("file.save", "Save"),
        Item("file.saveAs", "Save As..."),
        Item("file.saveAll", "Save All"),
        Sep,
        Item("file.revert", "Revert File"),
        Item("editor.close", "Close Editor"),
        Item("file.closeFolder", "Close Folder"),
        Item("remote.close", "Close Remote Connection"),
        Sep,
        Item("app.quit", "Exit"),
    ];
    static EDIT: &[Entry] = &[
        Item("editor.undo", "Undo"),
        Item("editor.redo", "Redo"),
        Sep,
        Item("editor.cut", "Cut"),
        Item("editor.copy", "Copy"),
        Item("editor.paste", "Paste"),
        Sep,
        Item("find", "Find"),
        Item("replace", "Replace"),
        Sep,
        Item("editor.comment", "Toggle Line Comment"),
    ];
    static SELECTION: &[Entry] = &[
        Item("editor.selectAll", "Select All"),
        Item("editor.selectLine", "Expand Line Selection"),
        Sep,
        Item("editor.copyUp", "Copy Line Up"),
        Item("editor.copyDown", "Copy Line Down"),
        Item("editor.moveUp", "Move Line Up"),
        Item("editor.moveDown", "Move Line Down"),
        Sep,
        Item("editor.nextMatch", "Add Next Occurrence"),
    ];
    static VIEW: &[Entry] = &[
        Item("palette", "Command Palette..."),
        Sep,
        Item("view.explorer", "Explorer"),
        Item("view.search", "Search"),
        Item("view.scm", "Source Control"),
        Item("view.run", "Run"),
        Item("view.extensions", "Extensions"),
        Item("view.remote", "Remote Explorer"),
        Sep,
        Item("view.sidebar", "Primary Side Bar"),
        Sep,
        Item("view.zoomIn", "Zoom In"),
        Item("view.zoomOut", "Zoom Out"),
        Item("view.zoomReset", "Reset Zoom"),
    ];
    static GO: &[Entry] = &[
        Item("editor.next", "Next Editor"),
        Item("editor.prev", "Previous Editor"),
        Sep,
        Item("quickOpen", "Go to File..."),
        Item("gotoLine", "Go to Line/Column..."),
    ];
    static RUN: &[Entry] = &[Disabled("run.start", "Start Debugging"), Disabled("run.noDebug", "Run Without Debugging")];
    static TERMINAL: &[Entry] = &[Disabled("terminal.new", "New Terminal")];
    static HELP: &[Entry] = &[Item("palette", "Show All Commands"), Sep, Disabled("help.about", "About")];
    match index {
        0 => FILE,
        1 => EDIT,
        2 => SELECTION,
        3 => VIEW,
        4 => GO,
        5 => RUN,
        6 => TERMINAL,
        _ => HELP,
    }
}

pub fn disabled_key(id: &str) -> &'static str {
    match id {
        "run.start" => "F5",
        "run.noDebug" => "Ctrl+F5",
        "terminal.new" => "Ctrl+Shift+`",
        _ => "",
    }
}
