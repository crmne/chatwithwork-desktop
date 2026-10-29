//! The window's user agent: the platform web view's, plus a comment that
//! tells the server it's Chat with Work Desktop (see docs/desktop-app.md).
//! The phone apps send `Chat with Work; platform=ios; ...`; this never
//! matches their pattern, so the server shows the desktop the web app's full
//! layout.
//!
//! The token is a comment, in parentheses, because WebKitGTK refuses a user
//! agent that isn't RFC 7231's products and comments
//! (`isValidUserAgentHeaderValue`) and keeps its own: `Desktop;` outside
//! parentheses is not a product.

/// What this build can do for the page, as `bridge-components`. Add a name
/// when the page API gains something the server should know about.
pub const COMPONENTS: [&str; 7] = [
    "external",
    "preferences",
    "auth-session",
    "downloads",
    "computer",
    "scan",
    "settings-pane-v1",
];

pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// `0.3.1` → 301: major × 10,000 + minor × 100 + patch.
pub fn build_number(version: &str) -> u32 {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    major * 10_000 + minor.min(99) * 100 + patch.min(99)
}

pub fn token(version: &str) -> String {
    format!(
        "(Chat with Work Desktop; platform={}; version={version}; build={}; bridge-components: [{}])",
        platform(),
        build_number(version),
        COMPONENTS.join(" ")
    )
}

/// The web view's own user agent on each platform, which Tauri can't read
/// before the web view exists, so the token can be appended to it.
fn base() -> &'static str {
    if cfg!(target_os = "macos") {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"
    } else if cfg!(windows) {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0"
    } else if cfg!(target_arch = "aarch64") {
        "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"
    } else {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"
    }
}

pub fn user_agent(version: &str) -> String {
    format!("{} {}", base(), token(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_names_the_desktop_its_platform_version_and_components() {
        let ua = user_agent("0.4.0");
        assert!(ua.starts_with("Mozilla/5.0 ("));
        assert!(ua.ends_with(&format!(
            "(Chat with Work Desktop; platform={}; version=0.4.0; build=400; bridge-components: [external preferences auth-session downloads computer scan settings-pane-v1])",
            platform()
        )));
    }

    /// The server's phone pattern is `Chat with Work; platform=(ios|android)`.
    #[test]
    fn it_never_reads_as_a_phone_app() {
        let ua = user_agent("1.2.3");
        assert!(!ua.contains("Chat with Work; platform="));
        assert!(!ua.contains("Hotwire Native") && !ua.contains("Turbo Native"));
    }

    /// RFC 7231: products (`token["/"token]`) and comments in parentheses,
    /// separated by spaces, as WebKitGTK checks before it takes one.
    #[test]
    fn it_is_a_valid_user_agent_header() {
        fn token_char(c: char) -> bool {
            c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
        }
        let ua = user_agent("0.3.1");
        let mut rest = ua.as_str();
        while !rest.is_empty() {
            rest = rest.trim_start_matches(' ');
            if let Some(comment) = rest.strip_prefix('(') {
                let end = comment.find(')').expect("a comment closes");
                assert!(!comment[..end].contains(['(', '\\']));
                rest = &comment[end + 1..];
            } else {
                let end = rest.find(' ').unwrap_or(rest.len());
                let product = &rest[..end];
                let (name, version) = product.split_once('/').unwrap_or((product, "x"));
                assert!(
                    !name.is_empty() && name.chars().all(token_char),
                    "{product}"
                );
                assert!(
                    !version.is_empty() && version.chars().all(token_char),
                    "{product}"
                );
                rest = &rest[end..];
            }
        }
    }

    #[test]
    fn builds_count_up_with_versions() {
        assert_eq!(build_number("0.3.1"), 301);
        assert_eq!(build_number("1.2.0"), 10_200);
        assert_eq!(build_number("1.2.0-rc.1"), 10_200);
        assert!(build_number("0.10.0") > build_number("0.9.99"));
    }
}
