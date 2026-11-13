//! Language modes: status bar name, line comment token and the explorer/tab glyph.

pub struct Lang {
    pub name: &'static str,
    pub comment: &'static str,
    pub glyph: &'static str,
    pub color: u32,
}

macro_rules! lang {
    ($name:expr, $comment:expr, $glyph:expr, $color:expr) => {
        Lang { name: $name, comment: $comment, glyph: $glyph, color: $color }
    };
}

pub static PLAIN: Lang = lang!("Plain Text", "", "≡", 0x8a8a8a);
static TABLE: &[(&[&str], Lang)] = &[
    (&["rs"], lang!("Rust", "//", "RS", 0xdea584)),
    (&["py", "pyw"], lang!("Python", "#", "PY", 0x3572a5)),
    (&["js", "mjs", "cjs"], lang!("JavaScript", "//", "JS", 0xcbcb41)),
    (&["jsx"], lang!("JavaScript JSX", "//", "JS", 0x519aba)),
    (&["ts", "mts", "cts"], lang!("TypeScript", "//", "TS", 0x519aba)),
    (&["tsx"], lang!("TypeScript JSX", "//", "TS", 0x519aba)),
    (&["json", "jsonc"], lang!("JSON", "//", "{}", 0xcbcb41)),
    (&["toml"], lang!("TOML", "#", "⚙", 0x9c4221)),
    (&["yaml", "yml"], lang!("YAML", "#", "≡", 0xa074c4)),
    (&["md", "markdown"], lang!("Markdown", "", "M↓", 0x519aba)),
    (&["html", "htm"], lang!("HTML", "", "<>", 0xe37933)),
    (&["css"], lang!("CSS", "", "#", 0x519aba)),
    (&["scss", "sass"], lang!("SCSS", "//", "#", 0xf55385)),
    (&["c", "h"], lang!("C", "//", "C", 0x519aba)),
    (&["cpp", "cc", "cxx", "hpp", "hh"], lang!("C++", "//", "C+", 0xa074c4)),
    (&["cs"], lang!("C#", "//", "C#", 0x519aba)),
    (&["go"], lang!("Go", "//", "GO", 0x519aba)),
    (&["java"], lang!("Java", "//", "J", 0xcc3e44)),
    (&["kt", "kts"], lang!("Kotlin", "//", "K", 0xa074c4)),
    (&["swift"], lang!("Swift", "//", "SW", 0xe37933)),
    (&["rb"], lang!("Ruby", "#", "RB", 0xcc3e44)),
    (&["php"], lang!("PHP", "//", "PHP", 0xa074c4)),
    (&["sh", "bash", "zsh"], lang!("Shell Script", "#", "$_", 0x4d5a5e)),
    (&["ps1", "psm1"], lang!("PowerShell", "#", ">_", 0x519aba)),
    (&["bat", "cmd"], lang!("Batch", "REM", ">_", 0xcbcb41)),
    (&["sql"], lang!("SQL", "--", "DB", 0xf55385)),
    (&["lua"], lang!("Lua", "--", "LU", 0x519aba)),
    (&["xml", "svg"], lang!("XML", "", "<>", 0xe37933)),
    (&["ini", "cfg", "conf", "env"], lang!("Properties", "#", "⚙", 0x6d8086)),
    (&["slint"], lang!("Slint", "//", "SL", 0x2379f4)),
    (&["vue"], lang!("Vue", "//", "V", 0x8dc149)),
    (&["dockerfile"], lang!("Dockerfile", "#", "🐳", 0x519aba)),
    (&["log", "txt"], lang!("Plain Text", "", "≡", 0x8a8a8a)),
];

pub fn detect(file_name: &str) -> &'static Lang {
    let lower = file_name.to_ascii_lowercase();
    match lower.as_str() {
        "dockerfile" => return by_ext("dockerfile"),
        "makefile" => return &MAKE,
        "cargo.lock" => return by_ext("toml"),
        _ => {}
    }
    if lower.starts_with(".env") || lower == ".gitignore" || lower == "config" {
        return &PROPS;
    }
    by_ext(lower.rsplit_once('.').map(|(_, e)| e).unwrap_or(""))
}

fn by_ext(ext: &str) -> &'static Lang {
    TABLE.iter().find(|(exts, _)| exts.contains(&ext)).map(|(_, l)| l).unwrap_or(&PLAIN)
}

static MAKE: Lang = lang!("Makefile", "#", "M", 0xe37933);
static PROPS: Lang = lang!("Properties", "#", "⚙", 0x6d8086);

pub fn all() -> impl Iterator<Item = &'static Lang> {
    std::iter::once(&PLAIN).chain(TABLE.iter().map(|(_, l)| l))
}

pub fn color(c: u32) -> slint::Color {
    slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}
