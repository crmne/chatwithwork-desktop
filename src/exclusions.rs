//! Local custom exclusions. This editor cannot relax built-in protection.

use anyhow::{Result, bail, ensure};

use crate::config::DenyConfig;
use crate::policy::DenyList;

/// Validate using the same glob compiler the reader enforces.
pub fn pattern(value: &str) -> Result<String> {
    ensure!(
        value.len() <= 512 && !value.chars().any(char::is_control),
        "Use a pattern of at most 512 bytes, without control characters."
    );
    let value = value.trim();
    ensure!(
        !value.trim_matches('/').is_empty(),
        "Enter a file or folder pattern."
    );
    DenyList::new([value.to_owned()])?;
    Ok(value.to_owned())
}

/// Add, replace atomically, or remove an exact custom entry. No built-in
/// removal or hard-link setting is accepted, even from a local caller.
pub fn edit(
    config: &mut DenyConfig,
    previous: Option<&str>,
    replacement: Option<&str>,
) -> Result<()> {
    ensure!(
        previous.is_some() || replacement.is_some(),
        "Choose an exclusion to change."
    );
    let replacement = replacement.map(pattern).transpose()?;
    let index = match previous {
        Some(previous) => Some(config.extra.iter().position(|p| p == previous).ok_or_else(
            || anyhow::anyhow!("That custom exclusion changed. Refresh and try again."),
        )?),
        None => None,
    };
    if let Some(ref replacement) = replacement
        && config
            .extra
            .iter()
            .enumerate()
            .any(|(i, p)| Some(i) != index && p == replacement)
    {
        bail!("That custom exclusion already exists.");
    }
    match (index, replacement) {
        (Some(index), Some(pattern)) => config.extra[index] = pattern,
        (Some(index), None) => {
            config.extra.remove(index);
        }
        (None, Some(pattern)) => config.extra.push(pattern),
        (None, None) => unreachable!(),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_replacement_cannot_drop_an_existing_exclusion_or_other_rules() {
        let mut config = DenyConfig {
            extra: vec!["Clients/Private".into()],
            remove: vec!["*.pem".into()],
            allow_hardlinks: true,
        };
        for invalid in ["", "///", "[", "secret\nfiles"] {
            assert!(edit(&mut config, Some("Clients/Private"), Some(invalid)).is_err());
            assert_eq!(config.extra, ["Clients/Private"]);
        }
        edit(&mut config, Some("Clients/Private"), Some("  *.secret  ")).unwrap();
        assert_eq!(config.extra, ["*.secret"]);
        assert_eq!(config.remove, ["*.pem"]);
        assert!(config.allow_hardlinks);
        assert!(edit(&mut config, Some(".ssh"), None).is_err());
        assert!(edit(&mut config, None, Some("*.secret")).is_err());
        edit(&mut config, Some("*.secret"), None).unwrap();
        assert!(config.extra.is_empty());
    }
}
