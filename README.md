# rcode

A small code editor that copies the VS Code look and keyboard shortcuts, built with Rust + [Slint](https://slint.dev) (software renderer, no webview).

## Build and run (Windows)

1. Install Rust with the MSVC toolchain (`rustup`, plus "Desktop development with C++" from Visual Studio Build Tools).
2. In this folder:

```
cargo run --release              # empty window
cargo run --release -- C:\path\to\folder
```

The binary is `target\release\rcode.exe`.

## What works

- VS Code Dark Modern layout: custom title bar with menus and command center, activity bar, explorer, tabs, breadcrumbs, line numbers, status bar, notifications, dialogs.
- Quick Open `Ctrl+P`, Command Palette `Ctrl+Shift+P` / `F1`, Go to Line `Ctrl+G`, Find/Replace `Ctrl+F` / `Ctrl+H`.
- Editing shortcuts: `Ctrl+/`, `Alt+Up/Down`, `Shift+Alt+Up/Down`, `Ctrl+Shift+K`, `Ctrl+Enter`, `Ctrl+]` / `Ctrl+[`, `Ctrl+L`, `Ctrl+D`, `Ctrl+X/C` on an empty selection (whole line), smart `Home`, auto-indent, auto-closing brackets/quotes, Tab to the next tab stop.
- Tabs: `Ctrl+W`, `Ctrl+Tab`, `Ctrl+PageUp/PageDown`, `Ctrl+Shift+T`, middle click to close, save prompts.
- Remote-SSH (SFTP): hosts from `~/.ssh/config` (Remote Explorer, `Ctrl+Alt+O`, or the status bar button). Key files from `IdentityFile` or `~/.ssh/id_*` (passphrase prompt), then password. Unknown host keys are confirmed and saved to `known_hosts`; a changed key refuses the connection.

- Syntax highlighting with tree-sitter. Grammars are loaded at runtime from a `grammars/` folder next to the executable (or `$RCODE_GRAMMARS`), one subfolder per language with `parser.dll` (`.so` on Linux), `highlights.scm` and `config.txt`. Adding a language needs no rebuild of the editor. Build the folder with `python3 tools/build_grammars.py --target windows --out <dir>/grammars` (needs `zig`).

- Extensions: dynamic libraries in an `extensions/` folder next to the executable (or `$RCODE_EXTENSIONS`), turned on and off in the Extensions view (Ctrl+Shift+X). Only enabled ones are loaded. Extensions never draw: they return lists of prebuilt widgets (`sdk/`, crate `rcode-ext`). Bundled: Markdown Preview (Ctrl+K V).

Not yet: multiple cursors, terminal, search across files, git, ssh-agent, ProxyJump.

## Development

`cargo test` runs the editing and parser tests. `cargo run --features screenshot -- --screenshot <scene> out.png` renders a scene (`welcome`, `editor`, `palette`, `menu`, `find`, `dialog`, `remote`, `context`, `quickopen`) to a PNG without a display.
