//! Answers are Markdown (CommonMark with GitHub's tables and
//! strikethrough). Raw HTML in them is shown as text, never parsed, so
//! nothing an answer says can run in the window.

use pulldown_cmark::{Event, Options, Parser, html};

pub fn to_html(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let events = Parser::new_ext(markdown, options).map(|event| match event {
        Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
        other => other,
    });
    let mut out = String::with_capacity(markdown.len() * 3 / 2);
    html::push_html(&mut out, events);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_raw_html() {
        let html = to_html("Hi <script>alert(1)</script> **there**");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("<strong>there</strong>"));
    }

    #[test]
    fn renders_tables() {
        assert!(to_html("| a | b |\n|---|---|\n| 1 | 2 |").contains("<table>"));
    }
}
