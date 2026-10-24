//! Which pages the person allowed. The extension checks the same rules
//! (`browser/src/sites.ts`); keep them identical.
//!
//! - A host (`github.com`) allows it and its subdomains over HTTPS, on the
//!   default port.
//! - An origin (`http://localhost:3000`) allows exactly that scheme, host
//!   and port, without subdomains.
//! - Nothing else: no `file:`, `data:`, `chrome:` or extension pages, and no
//!   URL with a user name or password in it.

use url::Url;

pub fn allowed(page: &str, sites: &[String]) -> bool {
    let Ok(page) = Url::parse(page) else {
        return false;
    };
    if !matches!(page.scheme(), "https" | "http") {
        return false;
    }
    if !page.username().is_empty() || page.password().is_some() {
        return false;
    }
    sites.iter().any(|site| matches(&page, site))
}

fn matches(page: &Url, site: &str) -> bool {
    let entry = site.trim().to_lowercase();
    if entry.is_empty() {
        return false;
    }
    if entry.contains("://") {
        let Ok(origin) = Url::parse(&entry) else {
            return false;
        };
        return matches!(origin.scheme(), "https" | "http")
            && origin.scheme() == page.scheme()
            && origin.host_str() == page.host_str()
            && origin.port() == page.port();
    }
    let Some(host) = host_of(&entry) else {
        return false;
    };
    let Some(page_host) = page.host_str() else {
        return false;
    };
    page.scheme() == "https"
        && page.port().is_none()
        && (page_host == host || page_host.ends_with(&format!(".{host}")))
}

/// The host an entry names, as a URL gives it (punycode, lowercase), or
/// `None` for something that isn't one.
pub fn host_of(entry: &str) -> Option<String> {
    if entry
        .chars()
        .any(|c| "/?#@:*\\".contains(c) || c.is_whitespace())
    {
        return None;
    }
    let host = Url::parse(&format!("https://{entry}"))
        .ok()?
        .host_str()?
        .to_string();
    (host.contains('.') || host == "localhost").then_some(host)
}

/// A site entry as it is saved: a host in its URL form, or an origin
/// without a path. `None` for something that can't be allowed.
pub fn normalize(entry: &str) -> Option<String> {
    let entry = entry.trim().to_lowercase();
    if entry.contains("://") {
        let url = Url::parse(&entry).ok()?;
        if !matches!(url.scheme(), "https" | "http")
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let host = url.host_str()?;
        return Some(match url.port() {
            Some(port) => format!("{}://{host}:{port}", url.scheme()),
            None => format!("{}://{host}", url.scheme()),
        });
    }
    host_of(entry.trim_end_matches('/'))
}

/// The page's host, for messages and the audit log.
pub fn host(page: &str) -> String {
    Url::parse(page)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sites(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_host_allows_it_and_its_subdomains_over_https() {
        let s = sites(&["github.com"]);
        assert!(allowed("https://github.com/crmne", &s));
        assert!(allowed("https://gist.github.com/x", &s));
        assert!(!allowed("http://github.com/", &s));
        assert!(!allowed("https://github.com:8443/", &s));
        assert!(!allowed("https://notgithub.com/", &s));
        assert!(!allowed("https://github.com.evil.example/", &s));
    }

    #[test]
    fn an_origin_allows_exactly_itself() {
        let s = sites(&["http://localhost:3000"]);
        assert!(allowed("http://localhost:3000/chats", &s));
        assert!(!allowed("http://localhost:3001/", &s));
        assert!(!allowed("https://localhost:3000/", &s));
        assert!(!allowed("http://a.localhost:3000/", &s));
    }

    #[test]
    fn never_other_schemes_or_odd_entries() {
        assert!(!allowed("file:///etc/passwd", &sites(&["github.com"])));
        assert!(!allowed("chrome://settings", &sites(&["settings"])));
        assert!(!allowed(
            "https://user:pw@github.com/",
            &sites(&["github.com"])
        ));
        assert!(!allowed("https://github.com/", &sites(&["*.github.com"])));
        assert!(!allowed("https://github.com/", &sites(&["com"])));
        assert!(!allowed("https://github.com/", &sites(&[""])));
        assert!(!allowed("not a url", &sites(&["github.com"])));
    }

    #[test]
    fn hosts_compare_as_the_browser_writes_them() {
        assert!(allowed("https://GitHub.com/", &sites(&["GITHUB.COM"])));
        assert!(allowed(
            "https://xn--mnchen-3ya.de/",
            &sites(&["münchen.de"])
        ));
    }

    #[test]
    fn normalizes_what_the_person_types() {
        assert_eq!(normalize("GitHub.com").as_deref(), Some("github.com"));
        assert_eq!(normalize("github.com/").as_deref(), Some("github.com"));
        assert_eq!(
            normalize("münchen.de").as_deref(),
            Some("xn--mnchen-3ya.de")
        );
        assert_eq!(
            normalize("http://localhost:3000/").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            normalize("https://example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(normalize("https://example.com/path"), None);
        assert_eq!(normalize("*.github.com"), None);
        assert_eq!(normalize("ftp://example.com"), None);
        assert_eq!(normalize("com"), None);
    }
}
