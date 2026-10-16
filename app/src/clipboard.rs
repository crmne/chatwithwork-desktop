//! Copy an answer as HTML plus its original Markdown, as on the website.
//! Keep the clipboard owner alive on the UI thread and preserve click order.
use cww::account_settings::CopyFormat;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

pub fn answer(ctx: &egui::Context, markdown: String, format: CopyFormat) {
    if format == CopyFormat::Markdown {
        ctx.copy_text(markdown);
        return;
    }
    let html = html(&markdown);
    if !store(&html, &markdown) {
        ctx.copy_text(markdown);
    } else {
        // Egui flushes plain-copy commands after the frame. A later rich copy
        // must supersede an earlier selection or plain copy in that frame.
        ctx.output_mut(|out| {
            out.commands.retain(|command| {
                !matches!(
                    command,
                    egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_)
                )
            })
        });
    }
}

#[cfg(not(test))]
fn store(html: &str, markdown: &str) -> bool {
    thread_local! {
        // On Linux dropping this object can relinquish clipboard ownership.
        static CLIPBOARD: std::cell::RefCell<Option<arboard::Clipboard>> = const { std::cell::RefCell::new(None) };
    }
    CLIPBOARD.with(|slot| {
        let mut clipboard = slot.borrow_mut();
        if clipboard.is_none() {
            *clipboard = arboard::Clipboard::new().ok();
        }
        clipboard
            .as_mut()
            .is_some_and(|c| c.set_html(html, Some(markdown)).is_ok())
    })
}

#[cfg(test)]
thread_local! {
    pub static COPIED: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn store(html: &str, markdown: &str) -> bool {
    // UI tests must never read or replace the person's real clipboard.
    COPIED.with(|slot| *slot.borrow_mut() = Some((html.into(), markdown.into())));
    true
}

fn html(markdown: &str) -> String {
    let mut links = Vec::new();
    let mut images = Vec::new();
    let events = Parser::new_ext(
        markdown,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    )
    .filter_map(|event| match event {
        // The clipboard contains the Markdown structure, never executable
        // raw HTML or UI controls injected by the conversation renderer.
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Link { ref dest_url, .. }) => {
            let safe = safe_url(dest_url);
            links.push(safe);
            safe.then_some(event)
        }
        Event::End(TagEnd::Link) => links.pop().unwrap_or(false).then_some(event),
        Event::Start(Tag::Image { ref dest_url, .. }) => {
            let safe = safe_url(dest_url);
            images.push(safe);
            safe.then_some(event)
        }
        Event::End(TagEnd::Image) => images.pop().unwrap_or(false).then_some(event),
        Event::Start(Tag::CodeBlock(_)) => {
            Some(Event::Start(Tag::CodeBlock(CodeBlockKind::Indented)))
        }
        other => Some(other),
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    html = html
        .replace("<table>", "<table style=\"border-collapse:collapse\">")
        .replace("<pre>", "<pre style=\"font-family:monospace\">")
        .replace("<code>", "<code style=\"font-family:monospace\">");
    for tag in ["th", "td"] {
        let style = "border:1px solid #d0d7de;padding:4px 8px";
        html = html
            .replace(&format!("<{tag}>"), &format!("<{tag} style=\"{style}\">"))
            .replace(
                &format!("<{tag} style=\"text-align:"),
                &format!("<{tag} style=\"{style};text-align:"),
            );
    }
    html
}

fn safe_url(value: &str) -> bool {
    !value.chars().any(char::is_control)
        && (value.starts_with('/')
            || value.starts_with('#')
            || value.split_once(':').is_some_and(|(scheme, _)| {
                ["https", "http", "mailto"]
                    .iter()
                    .any(|safe| scheme.eq_ignore_ascii_case(safe))
            }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_answers_preserve_markdown_and_portable_document_structure() {
        let markdown = "**Summary**\n\n- One\n- Two\n\n[Source](https://example.com/report)\n\n| Name | Count |\n| :--- | ---: |\n| Alex | 3 |\n\n```ruby\nputs '<report>'\n```";
        answer(
            &egui::Context::default(),
            markdown.into(),
            CopyFormat::RichText,
        );
        let (html, plain) = COPIED.with(|slot| slot.borrow_mut().take().unwrap());
        assert_eq!(plain, markdown);
        for part in [
            "<strong>Summary</strong>",
            "<ul>",
            "<li>One</li>",
            "href=\"https://example.com/report\"",
            "border-collapse:collapse",
            "border:1px solid #d0d7de",
            "font-family:monospace",
            "&lt;report&gt;",
        ] {
            assert!(html.contains(part), "missing {part}: {html}");
        }
        assert!(!html.contains("class="));
    }

    #[test]
    fn raw_markup_and_unsafe_links_cannot_become_executable_clipboard_html() {
        let html = html(
            "<script>alert(1)</script>\n\n[run](javascript:alert) ![image](data:text/html,bad)\n\n<b onclick='run()'>text</b>\n\n[mail](mailto:alex@example.com)",
        );
        for unsafe_text in ["<script", "onclick", "javascript:", "data:text", "<b>"] {
            assert!(!html.contains(unsafe_text), "{html}");
        }
        assert!(html.contains("mailto:alex@example.com"));
    }

    #[test]
    fn markdown_choice_uses_the_plain_clipboard_output_only() {
        let ctx = egui::Context::default();
        COPIED.with(|slot| *slot.borrow_mut() = None);
        answer(&ctx, "**literal**".into(), CopyFormat::Markdown);
        assert!(ctx.output(|out| {
            out.commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == "**literal**"))
        }));
        assert!(COPIED.with(|slot| slot.borrow().is_none()));
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;
    #[test]
    fn later_copy_actions_win_within_one_frame() {
        let ctx = egui::Context::default();
        ctx.copy_text("older selection".into());
        answer(&ctx, "**latest**".into(), CopyFormat::RichText);
        assert!(ctx.output(|out| out.commands.is_empty()));
        assert_eq!(
            COPIED.with(|slot| slot.borrow().as_ref().unwrap().1.clone()),
            "**latest**"
        );
        answer(&ctx, "newer plain".into(), CopyFormat::Markdown);
        assert!(ctx.output(|out| matches!(out.commands.last(), Some(egui::OutputCommand::CopyText(text)) if text == "newer plain")));
    }
}
