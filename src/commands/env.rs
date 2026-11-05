//! A shared folder's environment (docs/secrets.md): `NAME=value` lines,
//! dotenv-style, that every command in the folder gets. A value is text,
//! or `$(command)`, which runs at the start of each run in that run's own
//! sandbox, with the folder's allowances. The list lives in this
//! computer's secret store, never in `config.toml`; it's set only here
//! (`cww roots env`), never by the server, which learns nothing of it. The
//! values are taken out of what the run prints, as Claude Code's token is.

use crate::auth::secrets::SecretStore;
use crate::coding::Secret;
use crate::config::Config;
use crate::paths::Paths;

/// Longest list accepted, in bytes. Windows' Credential Manager holds at
/// most 2,560 bytes an entry, and says so when it's more.
pub const MAX_TEXT: usize = 16 * 1024;
/// Most bytes a `$( )` may print, and a value may hold.
pub const MAX_OUTPUT: usize = 64 * 1024;
/// What a value becomes in what a run prints: never its name either, which
/// Chat with Work isn't told.
pub const REDACTED: &str = "[redacted]";

/// Variables a run sets itself, or that change how its programs start:
/// never a folder's. Compared without case, as Windows does.
const RESERVED: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "TERM",
    "SHELL",
    "USER",
    "USERNAME",
    "LOGNAME",
    "PWD",
    "IFS",
    "ENV",
    "BASH_ENV",
    "SHELLOPTS",
    "BASHOPTS",
    "PS4",
    "PROMPT_COMMAND",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "HOMEDRIVE",
    "HOMEPATH",
    "PSMODULEPATH",
    "PSMODULEANALYSISCACHEPATH",
    "PYTHONUTF8",
    "NO_UPDATE_NOTIFIER",
    "NPM_CONFIG_UPDATE_NOTIFIER",
    "RUSTUP_HOME",
    "CARGO_HOME",
    "GOROOT",
    "GOPATH",
    "DISABLE_AUTOUPDATER",
];

/// Prefixes of variables a run or a toolchain sets, or the loader reads.
const RESERVED_PREFIXES: &[&str] = &[
    "LD_",
    "DYLD_",
    "XDG_",
    "MISE_",
    "LC_",
    "CWW_",
    "CLAUDE_CODE_",
];

/// One part of a value: text, or a command whose output goes there.
#[derive(Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Command(String),
}

/// Never its text, which can be a secret, or a command holding one.
impl std::fmt::Debug for Part {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Text(_) => "Text([redacted])",
            Self::Command(_) => "Command([redacted])",
        })
    }
}

/// One `NAME=value` line. Its value never prints, in `Debug` either.
#[derive(Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub parts: Vec<Part>,
}

impl std::fmt::Debug for Variable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Variable({}=[redacted])", self.name)
    }
}

/// What's wrong with a line of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// A run's environment: the folder's variables as stored, and, once the
/// run has worked them out in its sandbox, their values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunEnv {
    pub variables: Vec<Variable>,
    pub values: Vec<(String, Secret)>,
}

/// Why `name` can't be a folder's variable, if it can't.
pub fn name_refusal(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("no name before =".into());
    }
    if name.chars().any(char::is_whitespace) {
        return Some(format!("{name:?}: a name has no spaces"));
    }
    let mut bytes = name.bytes();
    let shaped = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && name.len() <= 128;
    if !shaped {
        return Some(format!(
            "{name} isn't a variable's name: letters, digits and _, not starting with a digit"
        ));
    }
    let upper = name.to_ascii_uppercase();
    if RESERVED.contains(&upper.as_str()) || RESERVED_PREFIXES.iter().any(|p| upper.starts_with(p))
    {
        return Some(format!(
            "{name} is set by the sandbox itself; pick another name"
        ));
    }
    None
}

/// Read the list as dotenv does: `NAME=value` a line, blank lines and
/// `#` comments skipped, `export ` allowed before a name. A value is bare
/// (to the end of the line, less a ` #` comment and the spaces around it),
/// in single quotes (exactly the text, over lines too), or in double quotes
/// (over lines too, with `\n`, `\t`, `\"`, `\\` and `\$`). `$(command)` in
/// a bare or double-quoted value runs the command; in single quotes it's
/// text. Every mistake is named by its line.
pub fn parse(text: &str) -> Result<Vec<Variable>, Vec<LineError>> {
    let chars: Vec<char> = text.chars().collect();
    let mut parser = Parser {
        chars: &chars,
        i: 0,
        line: 1,
    };
    let mut variables: Vec<Variable> = Vec::new();
    let mut errors = Vec::new();
    while parser.i < chars.len() {
        let line = parser.line;
        match parser.variable() {
            Ok(None) => {}
            Ok(Some(variable)) => {
                // Without case, as a run sets them: a second spelling
                // would never reach the command.
                if variables
                    .iter()
                    .any(|v| v.name.eq_ignore_ascii_case(&variable.name))
                {
                    errors.push(LineError {
                        line,
                        message: format!("{} is set twice", variable.name),
                    });
                } else {
                    variables.push(variable);
                }
            }
            Err(message) => {
                errors.push(LineError { line, message });
                parser.skip_line();
            }
        }
    }
    if text.len() > MAX_TEXT {
        errors.push(LineError {
            line: parser.line,
            message: format!("the list is over {MAX_TEXT} bytes"),
        });
    }
    if errors.is_empty() {
        Ok(variables)
    } else {
        Err(errors)
    }
}

struct Parser<'a> {
    chars: &'a [char],
    i: usize,
    line: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r')) {
            self.i += 1;
        }
    }

    fn skip_line(&mut self) {
        while let Some(c) = self.next() {
            if c == '\n' {
                break;
            }
        }
    }

    /// The rest of a line after a value: only spaces or a comment.
    fn end_of_line(&mut self) -> Result<(), String> {
        self.skip_spaces();
        match self.peek() {
            None => Ok(()),
            Some('\n') => {
                self.next();
                Ok(())
            }
            Some('#') => {
                self.skip_line();
                Ok(())
            }
            Some(_) => Err("text after the closing quote".into()),
        }
    }

    fn variable(&mut self) -> Result<Option<Variable>, String> {
        while matches!(self.peek(), Some(' ' | '\t' | '\r')) {
            self.i += 1;
        }
        match self.peek() {
            None => return Ok(None),
            Some('\n') => {
                self.next();
                return Ok(None);
            }
            Some('#') => {
                self.skip_line();
                return Ok(None);
            }
            Some(_) => {}
        }
        let mut name = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => return Err("no = (write NAME=value)".into()),
                Some('=') => {
                    self.i += 1;
                    break;
                }
                Some(c) => {
                    name.push(c);
                    self.i += 1;
                }
            }
        }
        let name = name.trim();
        let name = name
            .strip_prefix("export ")
            .or_else(|| name.strip_prefix("export\t"))
            .map_or(name, str::trim_start)
            .to_string();
        if let Some(refusal) = name_refusal(&name) {
            return Err(refusal);
        }
        self.skip_spaces();
        let parts = match self.peek() {
            Some('\'') => {
                self.next();
                let mut text = String::new();
                loop {
                    match self.next() {
                        None => return Err("the ' isn't closed".into()),
                        Some('\'') => break,
                        Some(c) => text.push(c),
                    }
                }
                self.end_of_line()?;
                vec![Part::Text(text)]
            }
            Some('"') => {
                self.next();
                let parts = self.double_quoted()?;
                self.end_of_line()?;
                parts
            }
            _ => self.bare()?,
        };
        // No process's environment holds a NUL, nor does its command line.
        if parts.iter().any(|part| match part {
            Part::Text(text) | Part::Command(text) => text.contains('\0'),
        }) {
            return Err(format!("{name} holds a NUL character"));
        }
        Ok(Some(Variable { name, parts }))
    }

    fn double_quoted(&mut self) -> Result<Vec<Part>, String> {
        let mut parts = Vec::new();
        let mut text = String::new();
        loop {
            match self.next() {
                None => return Err("the \" isn't closed".into()),
                Some('"') => break,
                Some('\\') => match self.next() {
                    Some('n') => text.push('\n'),
                    Some('r') => text.push('\r'),
                    Some('t') => text.push('\t'),
                    Some(c @ ('"' | '\\' | '$')) => text.push(c),
                    Some(c) => {
                        text.push('\\');
                        text.push(c);
                    }
                    None => return Err("the \" isn't closed".into()),
                },
                Some('$') if self.peek() == Some('(') => {
                    self.next();
                    push_text(&mut parts, &mut text);
                    parts.push(Part::Command(self.command(true)?));
                }
                Some(c) => text.push(c),
            }
        }
        push_text(&mut parts, &mut text);
        Ok(parts)
    }

    fn bare(&mut self) -> Result<Vec<Part>, String> {
        let mut parts = Vec::new();
        let mut text = String::new();
        loop {
            match self.peek() {
                None => break,
                Some('\n') => {
                    self.next();
                    break;
                }
                // A comment only at the value's start or after a space: a
                // `#` right after a `$( )` is text.
                Some('#')
                    if (parts.is_empty() && text.is_empty()) || text.ends_with([' ', '\t']) =>
                {
                    self.skip_line();
                    break;
                }
                Some('$') if self.chars.get(self.i + 1) == Some(&'(') => {
                    self.i += 2;
                    push_text(&mut parts, &mut text);
                    parts.push(Part::Command(self.command(false)?));
                }
                Some(c) => {
                    text.push(c);
                    self.i += 1;
                }
            }
        }
        let end = text.trim_end().len();
        text.truncate(end);
        push_text(&mut parts, &mut text);
        Ok(parts)
    }

    /// A `$( )`'s command, after its `$(`, to the `)` that closes it:
    /// parentheses nest, and quotes in it hold what they quote.
    fn command(&mut self, lines: bool) -> Result<String, String> {
        let mut command = String::new();
        let mut depth = 0usize;
        let mut quote: Option<char> = None;
        loop {
            let Some(c) = self.peek() else {
                return Err("$( isn't closed".into());
            };
            if c == '\n' && !lines {
                return Err("$( isn't closed".into());
            }
            self.next();
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some('"'), c) if c == ESCAPE => {
                    command.push(c);
                    if let Some(escaped) = self.next() {
                        command.push(escaped);
                    }
                    continue;
                }
                (Some(_), _) => {}
                // Escaped by the shell that runs it (sh's `\`, PowerShell's
                // backtick): never the `)` that ends it.
                (None, c) if c == ESCAPE => {
                    command.push(c);
                    if let Some(escaped) = self.next() {
                        command.push(escaped);
                    }
                    continue;
                }
                (None, '\'' | '"') => quote = Some(c),
                (None, '(') => depth += 1,
                (None, ')') if depth == 0 => break,
                (None, ')') => depth -= 1,
                _ => {}
            }
            command.push(c);
        }
        let command = command.trim().to_string();
        if command.is_empty() {
            return Err("$( ) is empty".into());
        }
        Ok(command)
    }
}

/// What escapes the next character in the shell a `$( )` runs in.
const ESCAPE: char = if cfg!(windows) { '`' } else { '\\' };

fn push_text(parts: &mut Vec<Part>, text: &mut String) {
    if !text.is_empty() {
        parts.push(Part::Text(std::mem::take(text)));
    }
}

/// Where the secret store keeps folder `root`'s list: named for this
/// install too (by its config folder), since the keychain's entries are
/// shared by every install of the person's account, paired or not, and
/// another's folder can have the same ID.
pub fn entry(paths: &Paths, root: &str) -> String {
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        paths.config_dir.to_string_lossy().as_bytes(),
    );
    let install: String = digest.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("folder-env:{install}:{root}")
}

/// The store folders' lists are in: the keychain under this install's
/// names, whatever server is paired, or the 0600 file where this computer
/// keeps its keys there.
fn store(config: &Config, paths: &Paths) -> SecretStore {
    match SecretStore::for_config(config, paths) {
        store if store.in_keychain() => SecretStore::Keyring,
        file => file,
    }
}

/// Folder `root`'s list as stored, if it has one.
pub fn stored(config: &Config, paths: &Paths, root: &str) -> anyhow::Result<Option<String>> {
    store(config, paths).get(&entry(paths, root))
}

fn errors_text(errors: &[LineError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keep `text` as folder `root`'s list, once it reads without a mistake,
/// and save `config` noting it has one. If `config` can't be saved, the
/// list that was there before is put back. Empty, it [`clear`]s the list.
pub fn save(
    config: &mut Config,
    paths: &Paths,
    root: &str,
    text: &str,
) -> anyhow::Result<Vec<Variable>> {
    let variables = parse(text).map_err(|errors| anyhow::anyhow!(errors_text(&errors)))?;
    if variables.is_empty() {
        clear(config, paths, root)?;
        return Ok(variables);
    }
    let store = store(config, paths);
    let name = entry(paths, root);
    let before = store.get(&name)?;
    store.set(&name, text)?;
    let noted = config.folder_env.iter().any(|id| id == root);
    if !noted {
        config.folder_env.push(root.to_string());
    }
    if let Err(e) = config.save(paths) {
        if !noted {
            config.folder_env.retain(|id| id != root);
        }
        let put_back = match &before {
            Some(before) => store.set(&name, before),
            None => store.delete(&name),
        };
        if let Err(back) = put_back {
            tracing::warn!("couldn't put a folder's environment back: {back:#}");
        }
        return Err(e);
    }
    Ok(variables)
}

/// Forget folder `root`'s list: the note in `config` first, saved, so a
/// failed save never leaves a folder noted with no list, then the list.
pub fn clear(config: &mut Config, paths: &Paths, root: &str) -> anyhow::Result<()> {
    config.folder_env.retain(|id| id != root);
    config.save(paths)?;
    delete(config, paths, root)
}

/// Drop the note that folder `root` has a list, for a folder no longer
/// shared (the caller saves `config`), and delete the list, which only
/// logs a failure: a keychain that refuses never keeps a folder shared.
pub fn forget(config: &mut Config, paths: &Paths, root: &str) {
    config.folder_env.retain(|id| id != root);
    if let Err(e) = delete(config, paths, root) {
        tracing::warn!("couldn't delete a removed folder's environment: {e:#}");
    }
}

/// Delete folder `root`'s list wherever it can be: the store this
/// computer uses, and the 0600 file.
pub fn delete(config: &Config, paths: &Paths, root: &str) -> anyhow::Result<()> {
    let name = entry(paths, root);
    store(config, paths).delete(&name)?;
    SecretStore::File(paths.secrets_file()).delete(&name)
}

/// The variables of folder `root` for a run: none, without reading the
/// store, where `config` notes no list. A noted list that can't be read,
/// or reads with a mistake, stops the run: it would run without them.
pub fn for_run(config: &Config, paths: &Paths, root: &str) -> Result<Vec<Variable>, String> {
    if !config.folder_env.iter().any(|id| id == root) {
        return Ok(Vec::new());
    }
    let text = stored(config, paths, root)
        .map_err(|_| {
            "this folder's environment couldn't be read from this computer's secret store"
                .to_string()
        })?
        .ok_or_else(|| {
            format!(
                "this folder's environment isn't in this computer's secret store any more; set \
                 it again with `cww roots env {root} --stdin`, or `--clear` it"
            )
        })?;
    parse(&text).map_err(|errors| {
        format!(
            "this folder's environment has a mistake on this computer ({}); fix it with \
             `cww roots env {root} --stdin`",
            errors[0]
        )
    })
}

impl RunEnv {
    /// Add the values, last, each in its variable, never over one the run
    /// already sets (names are refused that could be; this holds whatever
    /// a toolchain adds).
    pub fn add_to(&self, env: &mut Vec<(String, String)>) {
        for (name, value) in &self.values {
            if !env.iter().any(|(set, _)| set.eq_ignore_ascii_case(name)) {
                env.push((name.clone(), value.expose().to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Vec<Part> {
        vec![Part::Text(value.into())]
    }

    fn one(line: &str) -> Variable {
        let mut variables = parse(line).unwrap_or_else(|e| panic!("{line:?}: {e:?}"));
        assert_eq!(variables.len(), 1, "{line:?}");
        variables.remove(0)
    }

    fn errors(list: &str) -> Vec<String> {
        parse(list)
            .expect_err(list)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn values_are_read_as_dotenv_reads_them() {
        assert_eq!(one("API_KEY=your-key").parts, text("your-key"));
        assert_eq!(
            one("export  API_KEY = spaced out  ").parts,
            text("spaced out")
        );
        assert_eq!(one("A=x # a comment").parts, text("x"));
        assert_eq!(one("A=x#not-a-comment").parts, text("x#not-a-comment"));
        assert_eq!(one("A=").parts, Vec::new());
        assert_eq!(
            one("A='$(not run) # kept'").parts,
            text("$(not run) # kept")
        );
        assert_eq!(
            one(r#"A="a\nb \"q\" \$(no)" # c"#).parts,
            text("a\nb \"q\" $(no)")
        );
        assert_eq!(one("A=\"two\nlines\"").parts, text("two\nlines"));
        assert_eq!(one("A='two\nlines'\r\n").parts, text("two\nlines"));
        assert_eq!(
            one("DB_PASSWORD=$(op read op://Vault/Item/password)").parts,
            vec![Part::Command("op read op://Vault/Item/password".into())]
        );
        assert_eq!(
            one(r#"A="pre-$(echo "a)b" | tr a c)-post""#).parts,
            vec![
                Part::Text("pre-".into()),
                Part::Command(r#"echo "a)b" | tr a c"#.into()),
                Part::Text("-post".into()),
            ]
        );
        assert_eq!(
            one("A=$(echo $(date))").parts,
            vec![Part::Command("echo $(date)".into())]
        );
        let list =
            "# tokens\n\nAPI_KEY=your-key\r\nDB_PASSWORD=$(op read op://Vault/Item/password)\n";
        let names: Vec<String> = parse(list).unwrap().into_iter().map(|v| v.name).collect();
        assert_eq!(names, ["API_KEY", "DB_PASSWORD"]);
    }

    #[test]
    fn every_mistake_is_named_by_its_line() {
        assert_eq!(
            errors("OK=1\nno equals here\nMY KEY=x\nB=$(op read\nC=\"open\n"),
            [
                "line 2: no = (write NAME=value)",
                "line 3: \"MY KEY\": a name has no spaces",
                "line 4: $( isn't closed",
                "line 5: the \" isn't closed",
            ]
        );
        assert_eq!(errors("=x"), ["line 1: no name before ="]);
        assert_eq!(errors("A=$( )"), ["line 1: $( ) is empty"]);
        assert_eq!(errors("A='x' y"), ["line 1: text after the closing quote"]);
        assert_eq!(errors("A=1\nA=2"), ["line 2: A is set twice"]);
        assert!(errors("1A=x")[0].contains("isn't a variable's name"));
    }

    #[test]
    fn names_the_sandbox_sets_are_refused() {
        for name in [
            "PATH",
            "path",
            "HOME",
            "TMPDIR",
            "BASH_ENV",
            "LD_PRELOAD",
            "DYLD_X",
            "MISE_DATA_DIR",
            "XDG_CONFIG_HOME",
            "CWW_SECRET_STORE",
            "CLAUDE_CODE_TMPDIR",
            "SystemRoot",
        ] {
            assert!(
                name_refusal(name).is_some_and(|r| r.contains("sandbox")),
                "{name}"
            );
        }
        for name in [
            "OP_SERVICE_ACCOUNT_TOKEN",
            "api_key",
            "_X",
            "RAILS_MASTER_KEY",
        ] {
            assert_eq!(name_refusal(name), None, "{name}");
        }
    }

    #[test]
    fn values_never_print_and_never_replace_the_runs_own() {
        let variable = one("A=hunter2");
        assert!(!format!("{variable:?}").contains("hunter2"));
        let run = RunEnv {
            variables: vec![variable],
            values: vec![
                ("A".into(), Secret::new("hunter2".into())),
                ("Path".into(), Secret::new("/evil".into())),
            ],
        };
        assert!(!format!("{run:?}").contains("hunter2"));
        let mut env = vec![("PATH".to_string(), "/usr/bin".to_string())];
        run.add_to(&mut env);
        assert_eq!(
            env,
            [
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("A".to_string(), "hunter2".to_string()),
            ]
        );
    }

    #[test]
    fn a_list_is_kept_in_the_store_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::under(dir.path());
        paths.ensure().unwrap();
        let mut config = Config {
            secret_store: Some("file".into()),
            ..Default::default()
        };
        assert!(save(&mut config, &paths, "app", "PATH=/x").is_err());
        assert_eq!(for_run(&config, &paths, "app").unwrap(), Vec::new());
        save(&mut config, &paths, "app", "A=1\nB=$(echo 2)").unwrap();
        assert_eq!(config.folder_env, ["app"]);
        assert_eq!(for_run(&config, &paths, "app").unwrap().len(), 2);
        // Noted, but gone from the store: the run stops.
        SecretStore::File(paths.secrets_file())
            .delete(&entry(&paths, "app"))
            .unwrap();
        assert!(for_run(&config, &paths, "app").is_err());
        save(&mut config, &paths, "app", "A=1\nB=$(echo 2)").unwrap();
        assert!(
            !std::fs::read_to_string(paths.config_dir.join("config.toml"))
                .unwrap_or_default()
                .contains("A=1")
        );
        save(&mut config, &paths, "app", "# nothing\n").unwrap();
        assert_eq!(stored(&config, &paths, "app").unwrap(), None);
        assert!(config.folder_env.is_empty());
        save(&mut config, &paths, "app", "A=1").unwrap();
        forget(&mut config, &paths, "app");
        assert_eq!(stored(&config, &paths, "app").unwrap(), None);
        assert!(config.folder_env.is_empty());
    }

    #[test]
    fn names_differ_by_more_than_case() {
        assert_eq!(
            errors("API_KEY=1\napi_key=2"),
            ["line 2: api_key is set twice"]
        );
        assert!(parse("HOMEPATH=x").is_err());
        assert!(parse("HomeDrive=x").is_err());
    }

    #[test]
    fn a_nul_never_reaches_a_run() {
        assert_eq!(errors("A=x\0y"), ["line 1: A holds a NUL character"]);
        assert_eq!(
            errors("A=\"$(echo \0)\""),
            ["line 1: A holds a NUL character"]
        );
    }

    #[test]
    fn a_hash_right_after_a_command_is_text() {
        assert_eq!(
            one("A=pre$(printf foo)#suffix").parts,
            [
                Part::Text("pre".into()),
                Part::Command("printf foo".into()),
                Part::Text("#suffix".into())
            ]
        );
        assert_eq!(
            one("A=$(printf foo) # comment").parts,
            [Part::Command("printf foo".into())]
        );
        assert!(one("A= # nothing").parts.is_empty());
    }

    #[test]
    fn an_escaped_parenthesis_doesnt_end_a_command() {
        let escaped = format!("A=$(printf {ESCAPE}))");
        assert_eq!(
            one(&escaped).parts,
            [Part::Command(format!("printf {ESCAPE})"))]
        );
    }

    #[test]
    fn a_list_that_cant_be_noted_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::under(dir.path());
        paths.ensure().unwrap();
        let mut config = Config {
            secret_store: Some("file".into()),
            ..Default::default()
        };
        save(&mut config, &paths, "app", "A=1").unwrap();
        // A folder where config.toml should be: every save fails.
        let file = paths.config_dir.join("config.toml");
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert!(save(&mut config, &paths, "app", "A=2").is_err());
        assert_eq!(
            stored(&config, &paths, "app").unwrap().as_deref(),
            Some("A=1")
        );
        assert!(save(&mut config, &paths, "other", "B=1").is_err());
        assert_eq!(stored(&config, &paths, "other").unwrap(), None);
        assert_eq!(config.folder_env, ["app"]);
        // Cleared: the note goes first, so a failed save keeps both.
        assert!(clear(&mut config, &paths, "app").is_err());
        assert_eq!(
            stored(&config, &paths, "app").unwrap().as_deref(),
            Some("A=1")
        );
    }

    #[test]
    fn each_install_has_its_own_entries() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let (one, two) = (
            crate::paths::Paths::under(one.path()),
            crate::paths::Paths::under(two.path()),
        );
        assert_ne!(entry(&one, "app"), entry(&two, "app"));
        assert_eq!(entry(&one, "app"), entry(&one, "app"));
        assert!(entry(&one, "app").ends_with(":app"));
    }
}
