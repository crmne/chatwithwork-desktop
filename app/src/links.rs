//! Links that open in the app: `chatwithwork://<server>/<account>/chats/16`.
//!
//! The system hands the app any link with these schemes, from any web page
//! or program, so a link is hostile until it reads as exactly what the
//! server writes (docs/desktop-app.md, "Links"). It's a fixed grammar rather
//! than a URL to clean up: nothing is decoded, normalized or repaired, and
//! what doesn't match is dropped.
//!
//! A link only ever navigates the Chat with Work window to a page of the
//! server this computer is paired with. It approves nothing, runs nothing,
//! and gives the page it opens nothing the page wouldn't have by itself.

use url::Url;

use crate::server::Server;

/// Every Chat with Work Desktop answers all three; which one a link must use
/// depends on the server it names ([`Server::link_scheme`]), as one app can
/// pair with any server.
pub const SCHEMES: [&str; 3] = ["chatwithwork", "cww-staging", "cww-dev"];

/// Longer than any link the server writes, on the longest host a name can
/// have: 253 characters, with a port.
const MAX_LENGTH: usize = 512;

/// The sections a link may open. Each is a list, a record, or either.
const SECTIONS: [(&str, Record); 4] = [
    ("chats", Record::Required),
    ("decisions", Record::Never),
    ("bots", Record::Optional),
    ("projects", Record::Required),
];

/// A bot's page, saying to ask whether it may code here (`computer.setUpCoding`,
/// which asks in the app's own window). It moves the window and does nothing.
const SET_UP: &str = "set_up=coding";

#[derive(Clone, Copy, PartialEq)]
enum Record {
    Required,
    Optional,
    Never,
}

/// Whether a command line argument claims to be a link. Any case, with or
/// without slashes: what matters is that it's refused as an option.
pub fn is_link(argument: &str) -> bool {
    SCHEMES.iter().any(|scheme| {
        argument
            .as_bytes()
            .get(..=scheme.len())
            .is_some_and(|start| {
                start[..scheme.len()].eq_ignore_ascii_case(scheme.as_bytes())
                    && start[scheme.len()] == b':'
            })
    })
}

/// The page of `server` a link opens, or `None` for anything else.
pub fn page(server: &Server, link: &str) -> Option<Url> {
    if link.len() > MAX_LENGTH || !link.bytes().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }
    let rest = link
        .strip_prefix(server.link_scheme())?
        .strip_prefix("://")?;
    let (host, rest) = rest.split_once('/')?;
    if host != server.host() {
        return None;
    }
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let mut segments = path.split('/');
    let account = segments.next()?;
    let section = segments.next()?;
    let (_, kind) = SECTIONS.iter().find(|(name, _)| *name == section)?;
    let record = segments.next();
    if !digits(account, 7, 18) || segments.next().is_some() {
        return None;
    }
    let shown = match record {
        None => *kind != Record::Required,
        Some(number) => *kind != Record::Never && digits(number, 1, 18),
    };
    let query_ok = query.is_none_or(|query| {
        (section == "decisions" && decision(query))
            || (section == "bots" && record.is_some() && query == SET_UP)
    });
    if !shown || !query_ok {
        return None;
    }
    server.page(&format!("/{rest}"))
}

fn digits(text: &str, least: usize, most: usize) -> bool {
    (least..=most).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// `decision=t42`: the one thing a link's query says (`Decision#key`).
fn decision(query: &str) -> bool {
    query
        .strip_prefix("decision=")
        .and_then(|key| key.strip_prefix(['t', 'h']))
        .is_some_and(|id| digits(id, 1, 18))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::parse("https://work.example").unwrap()
    }

    fn opened(link: &str) -> Option<String> {
        page(&server(), link).map(|url| url.to_string())
    }

    #[test]
    fn a_long_host_is_a_host_like_any_other() {
        let host = format!("{}.example", vec!["a".repeat(60); 4].join("."));
        let server = Server::parse(&format!("https://{host}")).unwrap();
        assert!(
            page(
                &server,
                &format!("chatwithwork://{host}/482139075/chats/16")
            )
            .is_some()
        );
    }

    #[test]
    fn opens_the_pages_the_server_links_to() {
        for path in [
            "/482139075/chats/16",
            "/482139075/decisions",
            "/482139075/decisions?decision=t42",
            "/482139075/decisions?decision=h7",
            "/482139075/bots",
            "/482139075/bots/3",
            "/482139075/bots/3?set_up=coding",
            "/482139075/projects/12",
        ] {
            assert_eq!(
                opened(&format!("chatwithwork://work.example{path}")).as_deref(),
                Some(format!("https://work.example{path}").as_str()),
            );
        }
    }

    #[test]
    fn refuses_everything_else() {
        for link in [
            // Not a page, or nothing at all.
            "",
            "chatwithwork:",
            "chatwithwork://",
            "chatwithwork:///",
            "chatwithwork://work.example",
            "chatwithwork://work.example/",
            "chatwithwork://work.example/482139075",
            "chatwithwork://work.example/482139075/",
            "chatwithwork:/work.example/482139075/chats/16",
            "chatwithwork:work.example/482139075/chats/16",
            // The phone apps' callbacks use this scheme too.
            "chatwithwork://sign-in?token=abc",
            "chatwithwork://handoff?status=connected",
            // Another server, or this one with something added.
            "chatwithwork://evil.example/482139075/chats/16",
            "chatwithwork://work.example.evil.example/482139075/chats/16",
            "chatwithwork://evil.example#work.example/482139075/chats/16",
            "chatwithwork://evil.example?work.example/482139075/chats/16",
            "chatwithwork://work.example:8443/482139075/chats/16",
            "chatwithwork://WORK.EXAMPLE/482139075/chats/16",
            "chatwithwork://work%2eexample/482139075/chats/16",
            // Credentials.
            "chatwithwork://user:pass@work.example/482139075/chats/16",
            "chatwithwork://work.example@evil.example/482139075/chats/16",
            "chatwithwork://evil.example@work.example/482139075/chats/16",
            "chatwithwork://@work.example/482139075/chats/16",
            // Another scheme, inside the link or instead of it.
            "https://work.example/482139075/chats/16",
            "http://work.example/482139075/chats/16",
            "javascript:alert(1)",
            "javascript://work.example/482139075/chats/16",
            "chatwithwork:javascript:alert(1)",
            "chatwithwork://work.example/javascript:alert(1)",
            "chatwithwork://work.example/482139075/javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "chatwithwork://work.example/file:///etc/passwd",
            "chatwithwork://work.example//etc/passwd",
            "CHATWITHWORK://work.example/482139075/chats/16",
            // The scheme of another kind of server.
            "cww-staging://work.example/482139075/chats/16",
            "cww-dev://work.example/482139075/chats/16",
            // Open redirects and leaving the path.
            "chatwithwork://work.example//evil.example/482139075/chats/16",
            "chatwithwork://work.example/\\evil.example/482139075/chats/16",
            "chatwithwork://work.example/482139075/chats/16/../../../evil",
            "chatwithwork://work.example/482139075/../482139076/chats/16",
            "chatwithwork://work.example/482139075/./chats/16",
            "chatwithwork://work.example/482139075/chats/%2e%2e/%2e%2e/native",
            "chatwithwork://work.example/482139075/chats%2f16",
            "chatwithwork://work.example/482139075%2fchats/16",
            "chatwithwork://work.example/482139075/chats/16%00",
            "chatwithwork://work.example/482139075/chats/16%0d%0aSet-Cookie:x",
            // Whitespace and control characters the URL parser would strip.
            "chatwithwork://work.example/482139075/chats/1\t6",
            "chatwithwork://work.example/482139075/chats/1\n6",
            "chatwithwork://work.example/482139075/chats/16\r\n",
            " chatwithwork://work.example/482139075/chats/16",
            "chatwithwork://work.example/482139075/chats/16 ",
            "chatwithwork://work.example/482139075/chats/16\0",
            "chatwithwork://work.example/482139075/chats/16\u{2028}",
            "chatwithwork://wörk.example/482139075/chats/16",
            // Pages a link never opens: sign-in, flows, settings, marketing,
            // and where a browser lands after signing in (the server's list).
            "chatwithwork://work.example/482139075/chats",
            "chatwithwork://work.example/482139075/chats/new",
            "chatwithwork://work.example/482139075/projects",
            "chatwithwork://work.example/users/sign_in",
            "chatwithwork://work.example/users/auth/google_oauth2/callback",
            "chatwithwork://work.example/native/sign_ins/new",
            "chatwithwork://work.example/device?user_code=ABCD",
            "chatwithwork://work.example/share/abc",
            "chatwithwork://work.example/pricing",
            "chatwithwork://work.example/482139075/native/notification_browser_link?token=x",
            "chatwithwork://work.example/482139075/settings?tab=computer",
            "chatwithwork://work.example/482139075/apps/slack/installation",
            "chatwithwork://work.example/482139075/chats/16/files/1",
            "chatwithwork://work.example/482139075/chats/16/edit",
            "chatwithwork://work.example/482139075/bots/new",
            "chatwithwork://work.example/482139075/bots?set_up=coding",
            "chatwithwork://work.example/482139075/bots/3?set_up=everything",
            "chatwithwork://work.example/482139075/chats/3?set_up=coding",
            "chatwithwork://work.example/482139075/decisions/1",
            // Not an account, or not a record.
            "chatwithwork://work.example/482139/chats/16",
            "chatwithwork://work.example/1234567890123456789/chats/16",
            "chatwithwork://work.example/abcdefghi/chats/16",
            "chatwithwork://work.example/482139075/chats/abc",
            "chatwithwork://work.example/482139075/chats/1234567890123456789",
            "chatwithwork://work.example/482139075/chats/16/",
            // Words a page would read: a query fills the composer, a
            // fragment could scroll to anything.
            "chatwithwork://work.example/482139075/chats/new?text=hello",
            "chatwithwork://work.example/482139075/chats/new?url=https://evil.example",
            "chatwithwork://work.example/482139075/chats/16?approve=1",
            "chatwithwork://work.example/482139075/chats/16#x",
            "chatwithwork://work.example/482139075/decisions?decision=t42&approve=1",
            "chatwithwork://work.example/482139075/decisions?decision=x42",
            "chatwithwork://work.example/482139075/decisions?decision=t",
            "chatwithwork://work.example/482139075/decisions?",
            "chatwithwork://work.example/482139075/decisions?decision=t42#x",
        ] {
            assert_eq!(opened(link), None, "{link:?}");
        }
        let long = format!(
            "chatwithwork://work.example/482139075/chats/{}",
            "1".repeat(300)
        );
        assert_eq!(opened(&long), None);
    }

    #[test]
    fn each_kind_of_server_has_its_own_scheme() {
        let path = "/482139075/chats/16";
        for (origin, scheme, other) in [
            ("https://work.example", "chatwithwork", "cww-staging"),
            ("https://chatwithwork.com", "chatwithwork", "cww-dev"),
            (
                "https://staging.chatwithwork.com",
                "cww-staging",
                "chatwithwork",
            ),
            ("https://chatwithwork.localhost", "cww-dev", "chatwithwork"),
            ("http://localhost:3000", "cww-dev", "chatwithwork"),
            ("http://127.0.0.1:3000", "cww-dev", "cww-staging"),
        ] {
            let server = Server::parse(origin).unwrap();
            let host = server.host();
            assert_eq!(server.link_scheme(), scheme);
            assert_eq!(
                page(&server, &format!("{scheme}://{host}{path}"))
                    .unwrap()
                    .as_str(),
                format!("{origin}{path}")
            );
            assert_eq!(page(&server, &format!("{other}://{host}{path}")), None);
        }
    }

    #[test]
    fn a_link_is_told_from_an_option_in_any_spelling() {
        for link in [
            "chatwithwork://work.example/482139075/chats/16",
            "chatwithwork:",
            "CHATWITHWORK:x",
            "cww-staging:",
            "Cww-Dev://x",
        ] {
            assert!(is_link(link), "{link}");
        }
        for other in [
            "",
            "--server",
            "chatwithwork",
            "chatwithworks:",
            "é",
            "https://x",
        ] {
            assert!(!is_link(other), "{other}");
        }
    }
}
