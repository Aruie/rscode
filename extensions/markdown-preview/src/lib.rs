//! Markdown preview: turns a Markdown document into rcode's prebuilt widgets.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rcode_ext::{Action, CommandSpec, Contributes, Doc, Kind, PanelSpec, Widget};

const PANEL: &str = "preview";

#[derive(Default)]
pub struct MarkdownPreview;

impl rcode_ext::Extension for MarkdownPreview {
    fn activate(&mut self) -> Contributes {
        let md = vec!["Markdown".to_string()];
        Contributes {
            commands: vec![
                CommandSpec {
                    id: "openSide".into(),
                    title: "Markdown: Open Preview to the Side".into(),
                    key: "Ctrl+K V".into(),
                    editor_title_button: true,
                    languages: md.clone(),
                },
                CommandSpec {
                    id: "toggle".into(),
                    title: "Markdown: Toggle Preview".into(),
                    key: "Ctrl+Shift+V".into(),
                    editor_title_button: false,
                    languages: md.clone(),
                },
            ],
            panels: vec![PanelSpec { id: PANEL.into(), title: "Preview".into(), languages: md, ..Default::default() }],
        }
    }

    fn command(&mut self, id: &str, _doc: Option<&Doc>) -> Vec<Action> {
        match id {
            "openSide" => vec![Action::OpenPanel { panel: PANEL.into() }],
            "toggle" => vec![Action::OpenPanel { panel: PANEL.into() }],
            _ => Vec::new(),
        }
    }

    fn render(&mut self, _panel: &str, doc: &Doc) -> Vec<Widget> {
        to_widgets(&doc.text)
    }
}

rcode_ext::export_extension!(MarkdownPreview);

/// Escape text so the editor's inline markdown renders it literally.
fn escape(out: &mut String, s: &str) {
    for c in s.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
}

struct Ctx {
    out: Vec<Widget>,
    /// inline markdown being collected for the current text block
    buf: String,
    collecting: bool,
    quote: u8,
    /// one entry per open list: next number for ordered lists
    lists: Vec<Option<u64>>,
    /// marker of a list item whose text has not been emitted yet
    item: Option<String>,
    code: Option<(String, String)>,
    cells: Vec<String>,
    in_head: bool,
    heading: u8,
    links: Vec<String>,
}

impl Ctx {
    fn indent(&self) -> u8 {
        self.lists.len().saturating_sub(1) as u8
    }

    fn push(&mut self, mut w: Widget) {
        w.quote = self.quote;
        if w.kind != Kind::ListItem {
            w.indent = if self.lists.is_empty() { 0 } else { self.lists.len() as u8 };
        } else {
            w.indent = self.indent();
        }
        self.out.push(w);
    }

    /// Emit a pending list item (tight lists put text directly in the item).
    fn flush_item(&mut self) {
        if let Some(marker) = self.item.take() {
            let text = std::mem::take(&mut self.buf);
            self.collecting = false;
            self.push(Widget { marker, ..Widget::new(Kind::ListItem, text.trim()) });
        }
    }

    fn start_text(&mut self) {
        if self.item.is_none() {
            self.buf.clear();
        }
        self.collecting = true;
    }
}

pub fn to_widgets(text: &str) -> Vec<Widget> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut c = Ctx {
        out: Vec::new(),
        buf: String::new(),
        collecting: false,
        quote: 0,
        lists: Vec::new(),
        item: None,
        code: None,
        cells: Vec::new(),
        in_head: false,
        heading: 0,
        links: Vec::new(),
    };
    for ev in Parser::new_ext(text, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => c.start_text(),
                Tag::Heading { level, .. } => {
                    c.flush_item();
                    c.heading = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    c.start_text();
                }
                Tag::BlockQuote(_) => {
                    c.flush_item();
                    c.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    c.flush_item();
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    c.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    c.flush_item();
                    c.lists.push(start);
                }
                Tag::Item => {
                    c.flush_item();
                    let marker = match c.lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}.");
                            *n += 1;
                            m
                        }
                        _ => "•".to_string(),
                    };
                    c.item = Some(marker);
                    c.buf.clear();
                    c.collecting = true;
                }
                Tag::Table(_) => c.flush_item(),
                Tag::TableHead => {
                    c.in_head = true;
                    c.cells.clear();
                }
                Tag::TableRow => c.cells.clear(),
                Tag::TableCell => {
                    c.buf.clear();
                    c.collecting = true;
                }
                Tag::Emphasis => c.buf.push('*'),
                Tag::Strong => c.buf.push_str("**"),
                Tag::Strikethrough => c.buf.push_str("~~"),
                Tag::Link { dest_url, .. } => {
                    c.buf.push('[');
                    c.links.push(dest_url.into_string());
                }
                Tag::Image { .. } => c.buf.push_str("*\\[image: "),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if c.item.is_some() {
                        c.flush_item();
                    } else {
                        let t = std::mem::take(&mut c.buf);
                        c.collecting = false;
                        c.push(Widget::paragraph(t.trim()));
                    }
                }
                TagEnd::Heading(_) => {
                    let t = std::mem::take(&mut c.buf);
                    c.collecting = false;
                    let lvl = c.heading;
                    c.push(Widget::heading(lvl, t.trim()));
                }
                TagEnd::BlockQuote(_) => {
                    c.flush_item();
                    c.quote = c.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, mut body)) = c.code.take() {
                        if body.ends_with('\n') {
                            body.pop();
                        }
                        c.push(Widget::code(lang, body));
                    }
                }
                TagEnd::List(_) => {
                    c.flush_item();
                    c.lists.pop();
                }
                TagEnd::Item => c.flush_item(),
                TagEnd::TableCell => {
                    let t = std::mem::take(&mut c.buf);
                    c.collecting = false;
                    c.cells.push(t.trim().to_string());
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    let cells = std::mem::take(&mut c.cells);
                    let header = c.in_head;
                    c.in_head = false;
                    c.push(Widget { cells, header, ..Widget::new(Kind::TableRow, "") });
                }
                TagEnd::Emphasis => c.buf.push('*'),
                TagEnd::Strong => c.buf.push_str("**"),
                TagEnd::Strikethrough => c.buf.push_str("~~"),
                TagEnd::Link => {
                    let url = c.links.pop().unwrap_or_default();
                    c.buf.push_str("](");
                    c.buf.push_str(&url.replace(' ', "%20").replace(')', "%29"));
                    c.buf.push(')');
                }
                TagEnd::Image => c.buf.push_str("\\]*"),
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, body)) = &mut c.code {
                    body.push_str(&t);
                } else if c.collecting {
                    escape(&mut c.buf, &t);
                }
            }
            Event::Code(t) => {
                if c.collecting {
                    // backticks inside inline code can't be escaped; use a longer fence
                    let fence = if t.contains('`') { "`` " } else { "`" };
                    let close = if t.contains('`') { " ``" } else { "`" };
                    c.buf.push_str(fence);
                    c.buf.push_str(&t);
                    c.buf.push_str(close);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if c.collecting {
                    c.buf.push(' ');
                }
            }
            Event::Rule => {
                c.flush_item();
                c.push(Widget::new(Kind::Rule, ""));
            }
            Event::TaskListMarker(done) => {
                if let Some(m) = &mut c.item {
                    *m = if done { "☑".into() } else { "☐".into() };
                }
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                if c.collecting {
                    escape(&mut c.buf, &h);
                } else if !h.trim().is_empty() {
                    c.push(Widget::new(Kind::Label, h.trim_end()));
                }
            }
            _ => {}
        }
    }
    c.flush_item();
    c.out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks() {
        let md = "# Title *x*\n\nHello **world** and `code`.\n\n- a\n- b\n  1. c\n\n> quoted\n\n```py\nprint(1)\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n---\n- [x] done\n";
        let w = to_widgets(md);
        let kinds: Vec<Kind> = w.iter().map(|w| w.kind).collect();
        use Kind::*;
        assert_eq!(kinds, vec![Heading, Paragraph, ListItem, ListItem, ListItem, Paragraph, Code, TableRow, TableRow, Rule, ListItem]);
        assert_eq!(w[0].text, "Title *x*");
        assert_eq!(w[1].text, "Hello **world** and `code`\\.");
        assert_eq!((w[4].marker.as_str(), w[4].indent), ("1.", 1));
        assert_eq!(w[5].quote, 1);
        assert_eq!((w[6].lang.as_str(), w[6].text.as_str()), ("py", "print(1)"));
        assert!(w[7].header && w[7].cells == vec!["A", "B"]);
        assert_eq!(w[10].marker, "☑");
    }
}
