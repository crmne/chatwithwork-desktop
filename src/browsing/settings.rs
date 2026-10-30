//! Changing the browser settings in `config.toml`, the same way from the
//! command line and from the control channel.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::bridge::{Extension, Policy};
use super::sites;
use crate::config::{BotLoan, BrowserConfig, Config};

/// What the extension enforces: the allowed sites, and stopped while the
/// browser is disabled/stopped or the whole computer is paused. Disabling
/// also revokes calls already admitted by a tunnel or local client.
pub fn policy(config: &Config) -> Policy {
    Policy {
        sites: config.browser.sites.clone(),
        all_sites: config.browser.all_sites,
        blocked_sites: config.browser.blocked_sites.clone(),
        stopped: !config.browser.enabled || config.browser.stopped || config.paused,
        changes: config.browser.changes,
        auto_approve: !config.browser.ask_before_actions,
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

/// Add or remove a block without changing the allowed list or mode.
pub fn block_site(config: &mut Config, site: &str, blocked: bool) -> Result<String> {
    let Some(site) = sites::normalize(site) else {
        bail!("{site:?} isn't a site: write a host or an HTTP(S) origin");
    };
    if blocked {
        if !config.browser.blocked_sites.contains(&site) {
            config.browser.blocked_sites.push(site.clone());
            config.browser.blocked_sites.sort();
        }
    } else {
        let before = config.browser.blocked_sites.len();
        config.browser.blocked_sites.retain(|saved| saved != &site);
        if config.browser.blocked_sites.len() == before {
            bail!("{site} isn't on the blocked sites");
        }
    }
    Ok(site)
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
        "ask_before_actions": browser.ask_before_actions,
        "stopped": browser.stopped,
        "sites": browser.sites,
        "all_sites": browser.all_sites,
        "blocked_sites": browser.blocked_sites,
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
    fn old_config_stays_restricted_and_domain_saves_keep_every_character() {
        let mut config: Config =
            toml::from_str("[browser]\nenabled = true\nsites = [\"elster.de\"]\n").unwrap();
        assert!(!config.browser.all_sites);
        assert!(config.browser.ask_before_actions);
        assert!(config.browser.blocked_sites.is_empty());
        assert!(policy(&config).allows("https://elster.de/"));
        assert!(!policy(&config).allows("https://unlisted.example/"));
        allow_site(&mut config, "Elster.de").unwrap();
        block_site(&mut config, "private.elster.de", true).unwrap();
        block_site(&mut config, "PRIVATE.ELSTER.DE", true).unwrap();
        config.browser.all_sites = true;
        let saved = toml::to_string(&config).unwrap();
        let mut loaded: Config = toml::from_str(&saved).unwrap();
        assert_eq!(loaded.browser.sites, ["elster.de"]);
        assert_eq!(loaded.browser.blocked_sites, ["private.elster.de"]);
        assert!(policy(&loaded).allows("https://unlisted.example/"));
        assert!(!policy(&loaded).allows("https://private.elster.de/"));
        loaded.browser.all_sites = false;
        assert_eq!(loaded.browser.sites, ["elster.de"]);
        assert_eq!(loaded.browser.blocked_sites, ["private.elster.de"]);
        block_site(&mut loaded, "private.elster.de", false).unwrap();
        assert!(policy(&loaded).allows("https://private.elster.de/"));
        assert!(block_site(&mut loaded, "private.elster.de", false).is_err());
        assert!(block_site(&mut loaded, "file:///secret", true).is_err());
    }

    #[test]
    fn pausing_the_computer_stops_the_browser() {
        let mut config = Config::default();
        config.browser.enabled = true;
        assert!(!policy(&config).stopped);
        config.paused = true;
        assert!(policy(&config).stopped);
    }

    #[test]
    fn disabling_the_browser_revokes_existing_tool_instances() {
        let mut config = Config::default();
        config.browser.enabled = true;
        config.browser.changes = true;
        assert!(!policy(&config).stopped);
        config.browser.enabled = false;
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
