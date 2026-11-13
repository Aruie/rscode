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

Not yet: syntax highlighting, multiple cursors, terminal, search across files, git, ssh-agent, ProxyJump.

## Development

`cargo test` runs the editing and parser tests. `cargo run --features screenshot -- --screenshot <scene> out.png` renders a scene (`welcome`, `editor`, `palette`, `menu`, `find`, `dialog`, `remote`, `context`, `quickopen`) to a PNG without a display.
