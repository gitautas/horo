//! Matrix HTML (`formatted_body`) as blocks of styled text: what the React
//! app gets from its sanitiser and the WebView, done by hand for gpui.
//!
//! [`parse`] turns the HTML into [`Block`]s (paragraphs, quotes, code,
//! lists, headings), each paragraph one string with styled ranges over it,
//! which is what gpui's `StyledText` draws. Only the tags the Matrix spec
//! allows mean anything; anything else contributes its text. The reply
//! fallback (`<mx-reply>`) is dropped, since the reply is drawn from the
//! event's own relation. Pure, so it's tested here.

use std::ops::Range;

use matrix_sdk::ruma::html::{Html, NodeData, NodeRef};

/// How a run of text is styled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub spoiler: bool,
    /// A link or a mention; `Inline::links` says where to.
    pub link: bool,
}

/// One paragraph's worth of text with its styles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    /// Non-overlapping, in order; text outside them is plain.
    pub spans: Vec<(Range<usize>, Marks)>,
    pub links: Vec<(Range<usize>, String)>,
}

impl Inline {
    fn push(&mut self, text: &str, marks: Marks) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        if marks != Marks::default() {
            match self.spans.last_mut() {
                Some((range, last)) if *last == marks && range.end == start => range.end = self.text.len(),
                _ => self.spans.push((start..self.text.len(), marks)),
            }
        }
    }

    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Drop the whitespace HTML's line breaks leave at either end.
    fn trim(mut self) -> Inline {
        let start = self.text.len() - self.text.trim_start_matches([' ', '\t']).len();
        let end = self.text.trim_end_matches([' ', '\t', '\n']).len().max(start);
        if start == 0 && end == self.text.len() {
            return self;
        }
        let clip = |r: &Range<usize>| r.start.clamp(start, end) - start..r.end.clamp(start, end) - start;
        self.text = self.text[start..end].to_owned();
        self.spans = self.spans.iter().map(|(r, m)| (clip(r), *m)).filter(|(r, _)| !r.is_empty()).collect();
        self.links = self.links.iter().map(|(r, l)| (clip(r), l.clone())).filter(|(r, _)| !r.is_empty()).collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Inline),
    Heading(u8, Inline),
    Quote(Vec<Block>),
    Code(String),
    List { ordered: bool, start: u64, items: Vec<Vec<Block>> },
    Rule,
}

/// The blocks of a `formatted_body`.
pub fn parse(html: &str) -> Vec<Block> {
    let document = Html::parse(html);
    let mut builder = Builder::default();
    for child in document.children() {
        builder.node(&child, Marks::default());
    }
    builder.finish()
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    current: Inline,
}

impl Builder {
    /// Close the paragraph being written, if it has anything in it.
    fn flush(&mut self) {
        let inline = std::mem::take(&mut self.current);
        if !inline.is_blank() {
            self.blocks.push(Block::Paragraph(inline.trim()));
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush();
        self.blocks
    }

    /// The blocks of `node`'s children, built apart (a quote, a list item).
    fn nested(node: &NodeRef, marks: Marks) -> Vec<Block> {
        let mut inner = Builder::default();
        for child in node.children() {
            inner.node(&child, marks);
        }
        inner.finish()
    }

    fn node(&mut self, node: &NodeRef, marks: Marks) {
        match node.data() {
            NodeData::Text(text) => {
                // Whitespace collapses, as a browser would: Element puts
                // newlines between tags that mean nothing.
                let text = text.borrow();
                let mut collapsed = String::with_capacity(text.len());
                let mut space = self.current.text.ends_with([' ', '\n']) || self.current.text.is_empty();
                for c in text.chars() {
                    if c.is_whitespace() {
                        if !space {
                            collapsed.push(' ');
                            space = true;
                        }
                    } else {
                        collapsed.push(c);
                        space = false;
                    }
                }
                self.current.push(&collapsed, marks);
            }
            NodeData::Element(element) => {
                let tag = element.name.local.as_ref().to_ascii_lowercase();
                let attr = |name: &str| element.attrs.borrow().iter().find(|a| a.name.local.as_ref() == name).map(|a| a.value.to_string());
                match tag.as_str() {
                    "mx-reply" | "script" | "style" | "head" | "title" => {}
                    "br" => self.current.push("\n", marks),
                    "p" | "div" => {
                        self.flush();
                        self.children(node, marks);
                        self.flush();
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        self.flush();
                        let level = tag[1..].parse().unwrap_or(1);
                        let mut inner = Builder::default();
                        inner.children(node, Marks { bold: true, ..marks });
                        let inline = std::mem::take(&mut inner.current).trim();
                        if !inline.is_blank() {
                            self.blocks.push(Block::Heading(level, inline));
                        }
                    }
                    "blockquote" => {
                        self.flush();
                        let inner = Builder::nested(node, marks);
                        if !inner.is_empty() {
                            self.blocks.push(Block::Quote(inner));
                        }
                    }
                    "pre" => {
                        self.flush();
                        let mut text = String::new();
                        collect_text(node, &mut text);
                        self.blocks.push(Block::Code(text.trim_end_matches('\n').to_owned()));
                    }
                    "ul" | "ol" => {
                        self.flush();
                        let items = node
                            .children()
                            .filter(|c| c.as_element().is_some_and(|e| e.name.local.as_ref().eq_ignore_ascii_case("li")))
                            .map(|li| Builder::nested(&li, marks))
                            .collect();
                        let start = attr("start").and_then(|s| s.parse().ok()).unwrap_or(1);
                        self.blocks.push(Block::List { ordered: tag == "ol", start, items });
                    }
                    "hr" => {
                        self.flush();
                        self.blocks.push(Block::Rule);
                    }
                    "b" | "strong" => self.children(node, Marks { bold: true, ..marks }),
                    "i" | "em" => self.children(node, Marks { italic: true, ..marks }),
                    "u" => self.children(node, Marks { underline: true, ..marks }),
                    "s" | "del" | "strike" => self.children(node, Marks { strike: true, ..marks }),
                    "code" => self.children(node, Marks { code: true, ..marks }),
                    "a" => {
                        let start = self.current.text.len();
                        self.children(node, Marks { link: true, ..marks });
                        let end = self.current.text.len();
                        if let Some(href) = attr("href").filter(|_| end > start) {
                            self.current.links.push((start..end, href));
                        }
                    }
                    // Custom emoji and inline images: their name, until
                    // they're drawn as pictures.
                    "img" => {
                        let alt = attr("alt").or_else(|| attr("title")).unwrap_or_else(|| "🖼".into());
                        self.current.push(&alt, marks);
                    }
                    "span" if attr("data-mx-spoiler").is_some() => self.children(node, Marks { spoiler: true, ..marks }),
                    _ => self.children(node, marks),
                }
            }
            NodeData::Document | NodeData::Other => self.children(node, marks),
        }
    }

    fn children(&mut self, node: &NodeRef, marks: Marks) {
        for child in node.children() {
            self.node(&child, marks);
        }
    }
}

/// All the text under `node`, verbatim (for `<pre>`).
fn collect_text(node: &NodeRef, out: &mut String) {
    for child in node.children() {
        match child.data() {
            NodeData::Text(text) => out.push_str(&text.borrow()),
            NodeData::Element(e) if e.name.local.as_ref().eq_ignore_ascii_case("br") => out.push('\n'),
            _ => collect_text(&child, out),
        }
    }
}

/// Plain text for a block list (for tests and for copying).
#[cfg(test)]
fn plain(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(i) | Block::Heading(_, i) => i.text.clone(),
            Block::Quote(inner) => format!("> {}", plain(inner)),
            Block::Code(code) => format!("```{code}```"),
            Block::List { items, .. } => items.iter().map(|i| format!("- {}", plain(i))).collect::<Vec<_>>().join("\n"),
            Block::Rule => "---".into(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(blocks: &[Block]) -> &Inline {
        match &blocks[0] {
            Block::Paragraph(i) => i,
            other => panic!("not a paragraph: {other:?}"),
        }
    }

    #[test]
    fn inline_styles_become_ranges() {
        let blocks = parse("plain <b>bold <i>both</i></b> <code>x()</code>");
        let p = paragraph(&blocks);
        assert_eq!(p.text, "plain bold both x()");
        let bold = Marks { bold: true, ..Default::default() };
        assert_eq!(
            p.spans,
            vec![
                (6..11, bold),
                (11..15, Marks { italic: true, ..bold }),
                (16..19, Marks { code: true, ..Default::default() }),
            ]
        );
    }

    #[test]
    fn the_reply_fallback_is_dropped() {
        let blocks = parse("<mx-reply><blockquote>quoted <a href=\"x\">in reply to</a></blockquote></mx-reply>the answer");
        assert_eq!(plain(&blocks), "the answer");
    }

    #[test]
    fn links_keep_their_target() {
        let blocks = parse("hi <a href=\"https://matrix.to/#/@bob:x\">bob</a>!");
        let p = paragraph(&blocks);
        assert_eq!(p.text, "hi bob!");
        assert_eq!(p.links, vec![(3..6, "https://matrix.to/#/@bob:x".to_owned())]);
    }

    #[test]
    fn line_breaks_and_whitespace() {
        let blocks = parse("one<br>two\n   three<br/>");
        assert_eq!(paragraph(&blocks).text, "one\ntwo three");
    }

    #[test]
    fn blocks_nest() {
        let blocks = parse("<h2>title</h2><blockquote><p>wise</p></blockquote><ol start=\"3\"><li>a</li><li><b>b</b></li></ol><pre><code>let x = 1;\n  y</code></pre>");
        assert!(matches!(blocks[0], Block::Heading(2, _)));
        assert_eq!(plain(&blocks[1..2]), "> wise");
        match &blocks[2] {
            Block::List { ordered, start, items } => {
                assert!(*ordered);
                assert_eq!(*start, 3);
                assert_eq!(items.len(), 2);
            }
            other => panic!("not a list: {other:?}"),
        }
        assert_eq!(blocks[3], Block::Code("let x = 1;\n  y".into()));
    }

    #[test]
    fn spoilers_and_emoji_images() {
        let blocks = parse("<span data-mx-spoiler=\"\">secret</span> <img src=\"mxc://x/y\" alt=\":blob:\">");
        let p = paragraph(&blocks);
        assert_eq!(p.text, "secret :blob:");
        assert!(p.spans[0].1.spoiler);
    }
}
