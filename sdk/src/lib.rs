//! rcode extension interface.
//!
//! An extension is a dynamic library (`.dll` / `.so`) in `extensions/<id>/` next to the editor,
//! with an `extension.txt` manifest. The editor loads it only while it is enabled in the
//! Extensions view.
//!
//! The editor owns all drawing. An extension never draws: it answers requests with data, a list
//! of [`Widget`]s that the editor renders with its own prebuilt widgets, and [`Action`]s the editor
//! performs. Messages are JSON passed through three C functions:
//!
//! ```text
//! u32   rcode_ext_api_version(void)
//! u8*   rcode_ext_call(const u8* req, usize req_len, usize* out_len)   // returns a JSON Response
//! void  rcode_ext_free(u8* ptr, usize len)                              // frees a returned buffer
//! ```
//!
//! Write an extension by implementing [`Extension`] and calling [`export_extension!`].

use serde::{Deserialize, Serialize};

/// Bumped on incompatible protocol changes. The editor refuses other versions.
pub const API_VERSION: u32 = 1;

/// A document handed to the extension.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Doc {
    /// Full path (local) or remote path.
    pub path: String,
    /// Language mode name shown in the status bar, e.g. "Markdown".
    pub lang: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    /// Sent once when the extension is enabled. Answer with `contributes`.
    Activate { api: u32 },
    /// One of the extension's commands was run (palette, keybinding or editor title button).
    Command { id: String, doc: Option<Doc> },
    /// Fill a panel for a document. Sent when the panel opens and when the document changes.
    Render { panel: String, doc: Doc },
    /// A button widget was clicked.
    Click { panel: String, target: String, doc: Option<Doc> },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Response {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributes: Option<Contributes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widgets: Option<Vec<Widget>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Contributes {
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    #[serde(default)]
    pub panels: Vec<PanelSpec>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CommandSpec {
    pub id: String,
    /// Palette title, e.g. "Markdown: Open Preview to the Side".
    pub title: String,
    /// VS Code style keybinding: "Ctrl+Shift+V" or a chord starting with Ctrl+K: "Ctrl+K V".
    #[serde(default)]
    pub key: String,
    /// Show a button in the editor tab bar while the active document has one of these languages.
    #[serde(default)]
    pub editor_title_button: bool,
    /// Languages the command applies to; empty means any.
    #[serde(default)]
    pub languages: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    /// Beside the editor, like VS Code's "Open Preview to the Side".
    #[default]
    EditorSide,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PanelSpec {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub location: Location,
    /// The panel follows the active document while it has one of these languages.
    #[serde(default)]
    pub languages: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Heading,
    #[default]
    Paragraph,
    ListItem,
    Code,
    Rule,
    TableRow,
    Button,
    /// Plain text, no markup.
    Label,
}

/// One prebuilt widget. Text fields of `heading`, `paragraph`, `list_item` and table cells use
/// inline markdown: `**bold**`, `*italic*`, `` `code` ``, `~~strike~~`, `[link](target)`.
/// Links to `http(s)://` open in the browser; other targets open as files relative to the document.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Widget {
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Heading level 1-6.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub level: u8,
    /// Code block language (for highlighting), e.g. "python".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lang: String,
    /// List marker, e.g. "•", "1.", "☐".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub marker: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub header: bool,
    /// Button id sent back in `Request::Click`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Block quote nesting depth.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub quote: u8,
    /// Indentation steps (nested lists).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub indent: u8,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}

impl Widget {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        Widget { kind, text: text.into(), ..Default::default() }
    }
    pub fn heading(level: u8, text: impl Into<String>) -> Self {
        Widget { level, ..Self::new(Kind::Heading, text) }
    }
    pub fn paragraph(text: impl Into<String>) -> Self {
        Self::new(Kind::Paragraph, text)
    }
    pub fn code(lang: impl Into<String>, text: impl Into<String>) -> Self {
        Widget { lang: lang.into(), ..Self::new(Kind::Code, text) }
    }
    pub fn button(id: impl Into<String>, label: impl Into<String>) -> Self {
        Widget { id: id.into(), ..Self::new(Kind::Button, label) }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Open a panel as an editor tab; `beside` opens it in the group next to the active one.
    OpenPanel { panel: String, #[serde(default)] beside: bool },
    ClosePanel { panel: String },
    OpenUrl { url: String },
    OpenFile { path: String },
    /// level: 0 info, 1 warning, 2 error
    Notify { message: String, #[serde(default)] level: u8 },
}

/// Implement this and export it with [`export_extension!`].
pub trait Extension: Default {
    fn activate(&mut self) -> Contributes;
    fn command(&mut self, _id: &str, _doc: Option<&Doc>) -> Vec<Action> {
        Vec::new()
    }
    fn render(&mut self, _panel: &str, _doc: &Doc) -> Vec<Widget> {
        Vec::new()
    }
    fn click(&mut self, _panel: &str, _target: &str, _doc: Option<&Doc>) -> Vec<Action> {
        Vec::new()
    }
}

/// Handle one JSON request for an extension (used by the export macro).
pub fn dispatch<E: Extension>(ext: &mut E, req: &[u8]) -> Vec<u8> {
    let resp = match serde_json::from_slice::<Request>(req) {
        Err(e) => Response { error: Some(format!("bad request: {e}")), ..Default::default() },
        Ok(Request::Activate { api }) if api != API_VERSION => {
            Response { error: Some(format!("extension speaks API {API_VERSION}, editor speaks {api}")), ..Default::default() }
        }
        Ok(Request::Activate { .. }) => Response { contributes: Some(ext.activate()), ..Default::default() },
        Ok(Request::Command { id, doc }) => Response { actions: ext.command(&id, doc.as_ref()), ..Default::default() },
        Ok(Request::Render { panel, doc }) => Response { widgets: Some(ext.render(&panel, &doc)), ..Default::default() },
        Ok(Request::Click { panel, target, doc }) => Response { actions: ext.click(&panel, &target, doc.as_ref()), ..Default::default() },
    };
    serde_json::to_vec(&resp).unwrap_or_default()
}

/// Export the C entry points for an [`Extension`] type.
#[macro_export]
macro_rules! export_extension {
    ($t:ty) => {
        thread_local! {
            static __RCODE_EXT: ::std::cell::RefCell<$t> = ::std::cell::RefCell::new(<$t>::default());
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn rcode_ext_api_version() -> u32 {
            $crate::API_VERSION
        }

        /// # Safety
        /// `req` must point to `len` readable bytes and `out_len` must be writable.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn rcode_ext_call(req: *const u8, len: usize, out_len: *mut usize) -> *mut u8 {
            let input = unsafe { ::std::slice::from_raw_parts(req, len) };
            let out = __RCODE_EXT.with(|e| $crate::dispatch(&mut *e.borrow_mut(), input));
            let out = out.into_boxed_slice();
            unsafe { *out_len = out.len() };
            ::std::boxed::Box::into_raw(out) as *mut u8
        }

        /// # Safety
        /// `ptr`/`len` must come from `rcode_ext_call`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn rcode_ext_free(ptr: *mut u8, len: usize) {
            if !ptr.is_null() {
                drop(unsafe { ::std::boxed::Box::from_raw(::std::ptr::slice_from_raw_parts_mut(ptr, len)) });
            }
        }
    };
}
