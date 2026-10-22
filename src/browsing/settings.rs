//! Changing the browser settings in `config.toml`, the same way from the
//! command line and from the control channel.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::bridge::{Extension, Policy};
use super::sites;
use crate::config::{BotLoan, BrowserConfig, Config};

/// What the extension enforces: the allowed sites, and stopped while the
/// browser is stopped or the whole computer is paused.
pub fn policy(config: &Config) -> Policy {
    Policy {
        sites: config.browser.sites.clone(),
        stopped: config.browser.stopped || config.paused,
        changes: config.browser.changes,
    }
}

/// Allow a site. Returns it as saved.
pub fn allow_site(config: &mut Config, site: &str) -> Result<String> {
    let Some(site) = sites::normalize(site) else {
        bail!(
            "{site:?} isn't a site: write a host, like github.com (it and its subdomains, over \
             HTTPS), or an origin, like http://localhost:3000"
        );
    };
    if !config.browser.sites.contains(&site) {
        config.browser.sites.push(site.clone());
        config.browser.sites.sort();
    }
    Ok(site)
}

/// Stop allowing a site. Returns it as it was saved.
pub fn disallow_site(config: &mut Config, site: &str) -> Result<String> {
    let normalized = sites::normalize(site).unwrap_or_else(|| site.trim().to_lowercase());
    let before = config.browser.sites.len();
    config.browser.sites.retain(|s| *s != normalized);
    if config.browser.sites.len() == before {
        bail!("{normalized} isn't on the allowed sites");
    }
    Ok(normalized)
}

/// Lend the browser to a bot, or stop.
pub fn set_bot(config: &mut Config, bot: &str, name: Option<&str>, lent: bool) -> Result<()> {
    if bot.is_empty() || bot.len() > 20 || !bot.bytes().all(|b| b.is_ascii_digit()) {
        bail!(
            "{bot:?} isn't a bot's ID: it's the number in the bot's address in Chat with Work, like 42"
        );
    }
    let name: String = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| n.chars().take(80).collect())
        .or_else(|| config.browser.loan(bot).map(|loan| loan.name.clone()))
        .unwrap_or_default();
    config.browser.bots.retain(|loan| loan.id != bot);
    if lent {
        config.browser.bots.push(BotLoan {
            id: bot.to_string(),
            name,
            unattended: false,
        });
    }
    Ok(())
}

/// The browser settings for the control channel's `status` and `browser`.
pub fn status(browser: &BrowserConfig, extension: Option<Extension>) -> Value {
    json!({
        "enabled": browser.enabled,
        "changes": browser.changes,
        "stopped": browser.stopped,
        "sites": browser.sites,
        "bots": browser.bots,
        "connected": extension.is_some(),
        "extension": extension,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_and_disallows_sites_as_saved() {
        let mut config = Config::default();
        assert_eq!(
            allow_site(&mut config, "GitHub.com/").unwrap(),
            "github.com"
        );
        allow_site(&mut config, "github.com").unwrap();
        allow_site(&mut config, "http://localhost:3000").unwrap();
        assert_eq!(
            config.browser.sites,
            ["github.com", "http://localhost:3000"]
        );
        assert!(allow_site(&mut config, "*.example.com").is_err());
        assert_eq!(
            disallow_site(&mut config, "GITHUB.COM").unwrap(),
            "github.com"
        );
        assert!(disallow_site(&mut config, "github.com").is_err());
    }

    #[test]
    fn pausing_the_computer_stops_the_browser() {
        let mut config = Config::default();
        assert!(!policy(&config).stopped);
        config.paused = true;
        assert!(policy(&config).stopped);
    }

    #[test]
    fn lends_to_bots_by_id() {
        let mut config = Config::default();
        set_bot(&mut config, "42", Some("Researcher"), true).unwrap();
        assert_eq!(config.browser.loan("42").unwrap().name, "Researcher");
        set_bot(&mut config, "42", None, true).unwrap();
        assert_eq!(config.browser.loan("42").unwrap().name, "Researcher");
        set_bot(&mut config, "42", None, false).unwrap();
        assert!(config.browser.loan("42").is_none());
        assert!(set_bot(&mut config, "x", None, true).is_err());
    }
}
