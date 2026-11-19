# Roadmap

Decisions and deferred work, so nothing gets lost between sessions.

## Deferred

### Remote agent (rcode-server)
Small Rust binary uploaded over SFTP to `~/.rcode-server/<version>/` on connect, started over an SSH exec channel, speaking a request/response protocol over stdin/stdout. Falls back to plain SFTP when it cannot run (noexec home, policy).
1. Move file read/write/list to the agent (one round trip per request instead of open/stat/read/close).
2. Run search on the remote with the same in-process engine as local search (no `rg` needed there).
3. Later: file watching (explorer auto refresh), remote CPU/memory, formatter runs, terminal.
Build per remote arch (linux x86_64, aarch64) with zig.

### Multi-cursor (Ctrl+Alt+Up/Down, Ctrl+D adds the next match to the selection)
Slint's `TextInput` holds one cursor and one selection, so this needs our own editor widget (key input, carets, selections, clipboard, IME). The per-line colored rendering from syntax highlighting can be reused. Today Ctrl+D moves the selection to the next match instead of adding it.

### Formatter (Shift+Alt+F)
Run an external formatter on demand (ruff/black for Python, rustfmt, prettier...). Never always-on. For remote files, run it on the remote host.

### Font rendering
Text looks slightly rough on Windows with the software renderer (no ClearType). Need a zoomed screenshot and the display scale to decide; option: add the OpenGL (femtovg) renderer selectable by env var to compare.

### Faster remote file open
Until the agent exists: read files over an exec channel (`cat`) in one round trip instead of SFTP open/stat/read/close.

## Done
- VS Code layout, keybindings, quick open, palette, find/replace, explorer, tabs
- Remote-SSH over SFTP with ~/.ssh/config hosts
- Hangul IME shortcut mapping
- tree-sitter highlighting with runtime-loaded grammars
- Search view (Ctrl+Shift+F), title bar CPU/memory meter
- Extension host + Extensions view, Markdown Preview extension
