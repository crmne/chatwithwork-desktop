//! What a repository's code reaches when it runs: services on this computer
//! or the local network (a database, a cache), and the internet to install
//! its dependencies. Read from its files in the mirror with `git show`,
//! never by running any of its code, so a bot's setup can say it in its one
//! Allow and a prepare can tell when the folder doesn't allow it
//! (docs/coding-computers.md, "What it reaches").

use std::net::{IpAddr, ToSocketAddrs};
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::config::Root;
use crate::reach::{HostNetwork, Level, Where};

/// Most of a file read.
const MAX_READ: usize = 256 * 1024;

/// A service the code connects to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    /// "PostgreSQL".
    pub name: String,
    /// As the code names it: `localhost`, an address, or a Compose service.
    pub host: String,
    pub port: u16,
    #[serde(rename = "where")]
    pub place: Where,
    /// The file that says so: `docker-compose.yml`.
    pub from: String,
    /// The address this computer found for a name that isn't one (`db.lan`),
    /// which an exact `reach` entry needs. Never sent: only this computer
    /// uses it.
    #[serde(skip)]
    pub address: Option<IpAddr>,
}

/// What the code reaches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<Service>,
    /// Its installs need the internet.
    #[serde(default)]
    pub internet: bool,
    /// What it installs from the internet, for a person: "gems".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installs: Vec<String>,
    /// The files that say it needs the internet: `Gemfile`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internet_from: Vec<String>,
}

/// Well-known services: names an image or adapter goes by, the name a person
/// reads, and the port it serves on.
const KINDS: &[(&[&str], &str, u16)] = &[
    (
        &[
            "postgres",
            "postgis",
            "postgresql",
            "pgvector",
            "timescaledb",
        ],
        "PostgreSQL",
        5432,
    ),
    (&["mysql", "mariadb", "mysql2", "trilogy"], "MySQL", 3306),
    (&["redis", "valkey", "redis-stack", "rediss"], "Redis", 6379),
    (&["mongo", "mongodb"], "MongoDB", 27017),
    (&["memcached"], "Memcached", 11211),
    (&["elasticsearch", "opensearch"], "Elasticsearch", 9200),
    (&["rabbitmq", "amqp"], "RabbitMQ", 5672),
];

fn kind(word: &str) -> Option<(&'static str, u16)> {
    let word = word.to_ascii_lowercase();
    KINDS
        .iter()
        .find(|(names, ..)| names.iter().any(|n| word == *n))
        .map(|(_, name, port)| (*name, *port))
}

/// An image's own name: `docker.io/library/postgres:17-alpine` is `postgres`.
fn image_name(image: &str) -> &str {
    let last = image.rsplit('/').next().unwrap_or(image);
    last.split([':', '@']).next().unwrap_or(last)
}

/// Read what a repository's code reaches from its mirror's `HEAD`.
pub fn from_mirror(mirror: &Path, host: &HostNetwork) -> Needs {
    let names = top_level(mirror);
    read(&names, |name| show(mirror, name), host)
}

/// The names at the top of `HEAD`, and of `config/`.
fn top_level(mirror: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for (tree, prefix) in [("HEAD", ""), ("HEAD:config", "config/")] {
        if let Some(listing) = git(mirror, &["ls-tree", "--name-only", tree]) {
            names.extend(listing.lines().map(|n| format!("{prefix}{n}")));
        }
    }
    names
}

fn show(mirror: &Path, name: &str) -> Option<String> {
    let object = format!("HEAD:{name}");
    // Its size first, so a large file is never read into memory at all.
    let size: usize = git(mirror, &["cat-file", "-s", &object])?
        .trim()
        .parse()
        .ok()?;
    if size > MAX_READ {
        return None;
    }
    git(mirror, &["show", &object])
}

/// `git` on the bare mirror, reading only: no hooks, no filters run by
/// `show` on a bare repository, nothing asked for.
fn git(mirror: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(mirror)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut bytes = output.stdout;
    bytes.truncate(MAX_READ);
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// What the files named `names` say, each read with `file`.
pub fn read(names: &[String], file: impl Fn(&str) -> Option<String>, host: &HostNetwork) -> Needs {
    let mut needs = Needs::default();
    let mut compose_services: Vec<String> = Vec::new();
    let has = |name: &str| names.iter().any(|n| n == name);

    for name in names.iter().filter(|n| is_compose(n)) {
        if let Some(text) = file(name) {
            compose(&text, name, &mut needs, &mut compose_services);
        }
    }
    if has("config/database.yml")
        && let Some(text) = file("config/database.yml")
    {
        database_yml(&text, "config/database.yml", &mut needs);
    }
    for name in [
        ".env",
        ".env.example",
        ".env.sample",
        ".env.development",
        ".env.test",
        "Procfile",
        "Procfile.dev",
    ] {
        if has(name)
            && let Some(text) = file(name)
        {
            environment(&text, name, &mut needs);
        }
    }
    if has("package.json")
        && let Some(text) = file("package.json")
    {
        let package: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        if let Some(scripts) = package["scripts"].as_object() {
            for script in scripts.values().filter_map(|s| s.as_str()) {
                urls(script, "package.json", &mut needs);
            }
        }
        let has_deps = ["dependencies", "devDependencies"]
            .iter()
            .any(|key| package[key].as_object().is_some_and(|d| !d.is_empty()));
        if has_deps {
            install(&mut needs, "npm packages", "package.json");
        }
    }
    if has("Gemfile") {
        install(&mut needs, "gems", "Gemfile");
    }
    if has("Cargo.toml") && file("Cargo.toml").is_some_and(|t| t.contains("dependencies]")) {
        install(&mut needs, "crates", "Cargo.toml");
    }
    if let Some(name) = names
        .iter()
        .find(|n| n.starts_with("requirements") && n.ends_with(".txt"))
        .cloned()
        .or_else(|| has("pyproject.toml").then(|| "pyproject.toml".to_string()))
    {
        install(&mut needs, "Python packages", &name);
    }
    if has("go.mod") && file("go.mod").is_some_and(|t| t.contains("require")) {
        install(&mut needs, "Go modules", "go.mod");
    }

    // Placed last: a Compose service's name is this computer's.
    let mut placed = Vec::new();
    for mut service in std::mem::take(&mut needs.services) {
        let Some((place, address)) = place(&service.host, &compose_services, host) else {
            continue;
        };
        service.place = place;
        service.address = address;
        if !placed.iter().any(|s: &Service| {
            s.name == service.name && s.host == service.host && s.port == service.port
        }) {
            placed.push(service);
        }
    }
    needs.services = placed;
    needs
}

fn install(needs: &mut Needs, what: &str, from: &str) {
    needs.internet = true;
    if !needs.installs.iter().any(|w| w == what) {
        needs.installs.push(what.to_string());
    }
    if !needs.internet_from.iter().any(|f| f == from) {
        needs.internet_from.push(from.to_string());
    }
}

fn add(needs: &mut Needs, name: &str, host: &str, port: u16, from: &str) {
    needs.services.push(Service {
        name: name.to_string(),
        host: host.to_string(),
        port,
        place: Where::ThisComputer,
        from: from.to_string(),
        address: None,
    });
}

/// Where `host` is: this computer for `localhost` and Compose's services,
/// else as its address says. A name that isn't either is looked up; one
/// that doesn't resolve is left out.
fn place(host: &str, compose: &[String], network: &HostNetwork) -> Option<(Where, Option<IpAddr>)> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.eq_ignore_ascii_case("localhost") || compose.iter().any(|s| s == bare) {
        return Some((Where::ThisComputer, None));
    }
    if let Ok(address) = bare.parse::<IpAddr>() {
        return Some((network.classify(address), Some(address)));
    }
    if bare.is_empty() || bare.contains(['<', '%', '$', '{']) {
        return None;
    }
    let found: Vec<IpAddr> = (bare, 0).to_socket_addrs().ok()?.map(|a| a.ip()).collect();
    let address = *found
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| found.first())?;
    Some((network.classify(address), Some(address)))
}

fn is_compose(name: &str) -> bool {
    let yaml = name.ends_with(".yml") || name.ends_with(".yaml");
    yaml && !name.contains('/')
        && (name.starts_with("docker-compose") || name.starts_with("compose"))
}

/// A Compose file's services: each known image (or service name), on the
/// host port it's published on, else on its own port by its service name.
fn compose(text: &str, from: &str, needs: &mut Needs, names: &mut Vec<String>) {
    let Ok(value) = serde_saphyr::from_str::<serde_json::Value>(text) else {
        return;
    };
    let Some(services) = value["services"].as_object() else {
        return;
    };
    for (service, spec) in services {
        names.push(service.clone());
        let found = spec["image"]
            .as_str()
            .and_then(|image| kind(image_name(image)))
            .or_else(|| kind(service));
        let Some((name, port)) = found else {
            continue;
        };
        let published = spec["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|entry| published(entry, port));
        match published {
            Some(host_port) => add(needs, name, "localhost", host_port, from),
            None => add(needs, name, service, port, from),
        }
    }
}

/// The host port an entry of `ports:` publishes `target` on.
fn published(entry: &serde_json::Value, target: u16) -> Option<u16> {
    if let Some(spec) = entry.as_object() {
        let to = spec.get("target")?.as_u64()?;
        let on = spec.get("published")?;
        let on = on
            .as_u64()
            .or_else(|| on.as_str().and_then(|s| s.parse().ok()))?;
        return (to == u64::from(target))
            .then(|| u16::try_from(on).ok())
            .flatten();
    }
    let text = match entry {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let text = text.split('/').next()?;
    let parts: Vec<&str> = text.rsplitn(3, ':').collect();
    // "target", "published:target" or "ip:published:target", read backwards.
    let to: u16 = parts.first()?.parse().ok()?;
    if to != target {
        return None;
    }
    match parts.get(1) {
        Some(on) => on.parse().ok(),
        None => Some(to),
    }
}

/// A Rails `config/database.yml`: its adapter, host and port, read line by
/// line since its ERB isn't YAML. ERB's default (`{ "localhost" }`,
/// `|| "db"`) stands for the value; ERB without one means `localhost`.
fn database_yml(text: &str, from: &str, needs: &mut Needs) {
    let mut adapter = None;
    let mut host = None;
    let mut port = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("url:") {
            urls(value, from, needs);
        } else if let Some(value) = line.strip_prefix("adapter:") {
            adapter = adapter.or_else(|| kind(value.trim()));
        } else if let Some(value) = line.strip_prefix("host:") {
            host = host.or_else(|| erb_value(value));
        } else if let Some(value) = line.strip_prefix("port:") {
            port = port.or_else(|| erb_value(value).and_then(|p| p.parse().ok()));
        }
    }
    if let Some((name, default)) = adapter {
        let host = host.unwrap_or_else(|| "localhost".to_string());
        if !host.starts_with('/') {
            add(needs, name, &host, port.unwrap_or(default), from);
        }
    }
}

/// A `database.yml` value, plain or ERB. For ERB, the default it falls back
/// to: the last quoted string that isn't an environment variable's name
/// (`ENV.fetch("DB_HOST", "db")`, `ENV.fetch("DB_HOST") { "db" }`,
/// `ENV["DB_HOST"] || "db"`), else its last number (`ENV.fetch("DB_PORT",
/// 5432)`), else `localhost`.
fn erb_value(value: &str) -> Option<String> {
    let value = value.trim();
    if !value.contains("<%") {
        let plain = value.trim_matches(['"', '\'']);
        return (!plain.is_empty()).then(|| plain.to_string());
    }
    let mut quoted: Vec<&str> = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find(['"', '\'']) {
        let quote = rest[start..].chars().next()?;
        let after = &rest[start + 1..];
        let Some(end) = after.find(quote) else {
            break;
        };
        quoted.push(&after[..end]);
        rest = &after[end + 1..];
    }
    let default = quoted.iter().rev().find(|inner| {
        !inner.is_empty()
            && !inner
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    });
    if let Some(default) = default {
        return Some(default.to_string());
    }
    let number: String = rest
        .split(|c: char| !c.is_ascii_digit())
        .rfind(|n| !n.is_empty())
        .unwrap_or_default()
        .to_string();
    if !number.is_empty() {
        return Some(number);
    }
    Some("localhost".to_string())
}

/// `KEY=value` lines: service URLs (`DATABASE_URL=postgres://...`) and
/// `<NAME>_HOST` with `<NAME>_PORT`.
fn environment(text: &str, from: &str, needs: &mut Needs) {
    let mut hosts: Vec<(String, String)> = Vec::new();
    let mut ports: Vec<(String, u16)> = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches("export ").trim();
        urls(line, from, needs);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        if let Some(prefix) = key.strip_suffix("_HOST") {
            hosts.push((prefix.to_string(), value.to_string()));
        } else if let Some(prefix) = key.strip_suffix("_PORT")
            && let Ok(port) = value.parse()
        {
            ports.push((prefix.to_string(), port));
        }
    }
    for (prefix, host) in hosts {
        let Some(port) = ports
            .iter()
            .find(|(p, _)| *p == prefix)
            .map(|(_, port)| *port)
        else {
            continue;
        };
        let name = crate::reach::service(port)
            .map(str::to_string)
            .unwrap_or_else(|| format!("a service on port {port}"));
        add(needs, &name, &host, port, from);
    }
}

/// Service URLs in a piece of text: `postgres://user@db:5432/app`.
fn urls(text: &str, from: &str, needs: &mut Needs) {
    for word in text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | '`')) {
        let Some((scheme, _)) = word.split_once("://") else {
            continue;
        };
        let Some((name, port)) = kind(scheme.trim_end_matches("ql")).or_else(|| kind(scheme))
        else {
            continue;
        };
        let Ok(url) = url::Url::parse(word) else {
            continue;
        };
        let Some(host) = url.host_str().filter(|h| !h.is_empty()) else {
            continue;
        };
        add(needs, name, host, url.port().unwrap_or(port), from);
    }
}

/// Whether `root` lets its commands reach what `needs` names, on this
/// computer: local services for this computer's (where the computer can
/// give them), the exact address or the internet for the local network's,
/// and the internet for installs.
pub fn allowed(root: &Root, needs: &Needs) -> bool {
    let level = crate::commands::network_access(root);
    if needs.internet && level != Level::Internet {
        return false;
    }
    needs.services.iter().all(|service| match service.place {
        // Windows can't give a command this computer's services: asking
        // would never end.
        Where::ThisComputer => cfg!(windows) || level != Level::None,
        Where::LocalNetwork => {
            level == Level::Internet
                || reach_entry(service).is_some_and(|entry| root.reach.contains(&entry))
        }
        Where::Internet => level == Level::Internet,
    })
}

/// The exact `ip:port`s the local network's services need in `reach`.
pub fn reach_entries(needs: &Needs) -> Vec<String> {
    needs
        .services
        .iter()
        .filter(|s| s.place == Where::LocalNetwork)
        .filter_map(reach_entry)
        .collect()
}

/// A local-network service's `reach` entry: its address, the one this
/// computer found for its name when it has one.
fn reach_entry(service: &Service) -> Option<String> {
    let host = match service.address {
        Some(address) => address.to_string(),
        None => service.host.clone(),
    };
    crate::reach::parse_reach(&format!("{host}:{}", service.port))
        .ok()
        .map(|a| a.to_string())
}

/// What the setup's one Allow says of it: "It can reach PostgreSQL and
/// Redis on this computer, and the internet to install gems (from
/// docker-compose.yml and Gemfile)". `None` when it reaches nothing.
pub fn sentence(needs: &Needs) -> Option<String> {
    let names = |place: Where| -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        for service in needs.services.iter().filter(|s| s.place == place) {
            let shown = match place {
                Where::ThisComputer => service.name.clone(),
                _ => format!("{} at {}:{}", service.name, service.host, service.port),
            };
            if !found.contains(&shown) {
                found.push(shown);
            }
        }
        found
    };
    let mut parts = Vec::new();
    let here = names(Where::ThisComputer);
    if !here.is_empty() {
        parts.push(format!("{} on this computer", and_list(&here)));
    }
    let lan = names(Where::LocalNetwork);
    if !lan.is_empty() {
        parts.push(format!("{} on the local network", and_list(&lan)));
    }
    let away = names(Where::Internet);
    if !away.is_empty() {
        parts.push(format!("{} on the internet", and_list(&away)));
    }
    if needs.internet {
        parts.push(format!(
            "the internet to install {}",
            and_list(&needs.installs)
        ));
    }
    if parts.is_empty() {
        return None;
    }
    let mut files: Vec<String> = Vec::new();
    for file in needs
        .services
        .iter()
        .map(|s| s.from.clone())
        .chain(needs.internet_from.iter().cloned())
    {
        if !files.contains(&file) {
            files.push(file);
        }
    }
    let reached = match parts.as_slice() {
        [rest @ .., last] if !rest.is_empty() => format!("{}, and {last}", rest.join(", ")),
        _ => parts.join(""),
    };
    Some(format!(
        "It can reach {reached} (from {})",
        and_list(&files)
    ))
}

fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::Ipv4Addr;

    fn chalk() -> HostNetwork {
        HostNetwork {
            addresses: vec!["192.168.8.20".parse().unwrap()],
            bridges: vec![(Ipv4Addr::new(172, 17, 0, 0), 16)],
        }
    }

    fn needs_of(files: &[(&str, &str)]) -> Needs {
        let map: HashMap<String, String> = files
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect();
        let names: Vec<String> = map.keys().cloned().collect();
        read(&names, |n| map.get(n).cloned(), &chalk())
    }

    #[test]
    fn erb_defaults_are_read_in_each_form() {
        for (value, default) in [
            (r#"<%= ENV.fetch("DB_HOST", "db") %>"#, "db"),
            (
                r#"<%= ENV.fetch("DB_HOST") { "192.168.8.31" } %>"#,
                "192.168.8.31",
            ),
            (r#"<%= ENV["DB_HOST"] || "db" %>"#, "db"),
            (r#"<%= ENV.fetch("DB_PORT", 5432) %>"#, "5432"),
            (r#"<%= ENV.fetch("DB_PORT") { 6543 } %>"#, "6543"),
            (r#"<%= ENV["DB_HOST"] %>"#, "localhost"),
            ("db.internal", "db.internal"),
        ] {
            assert_eq!(erb_value(value).as_deref(), Some(default), "{value}");
        }
    }

    #[test]
    fn compose_services_are_found_on_the_port_they_are_published_on() {
        let needs = needs_of(&[(
            "docker-compose.yml",
            "services:\n  db:\n    image: postgres:17\n    ports:\n      - \"127.0.0.1:55432:5432\"\n  cache:\n    image: docker.io/library/redis:7-alpine\n  web:\n    build: .\n    ports: [\"3000:3000\"]\n",
        )]);
        let found: Vec<(&str, &str, u16)> = needs
            .services
            .iter()
            .map(|s| (s.name.as_str(), s.host.as_str(), s.port))
            .collect();
        assert!(
            found.contains(&("PostgreSQL", "localhost", 55432)),
            "{found:?}"
        );
        assert!(found.contains(&("Redis", "cache", 6379)), "{found:?}");
        assert!(
            needs
                .services
                .iter()
                .all(|s| s.place == Where::ThisComputer)
        );
        assert!(
            needs
                .services
                .iter()
                .all(|s| s.from == "docker-compose.yml")
        );
        assert_eq!(needs.services.len(), 2);
        assert!(!needs.internet);
    }

    #[test]
    fn long_port_syntax_is_read() {
        let entry: serde_json::Value = serde_json::json!({ "target": 6379, "published": "16379" });
        assert_eq!(published(&entry, 6379), Some(16379));
        assert_eq!(published(&serde_json::json!("5432"), 5432), Some(5432));
        assert_eq!(
            published(&serde_json::json!("5433:5432/tcp"), 5432),
            Some(5433)
        );
        assert_eq!(published(&serde_json::json!("8080:80"), 5432), None);
    }

    #[test]
    fn a_rails_database_and_env_files_name_their_hosts() {
        let needs = needs_of(&[
            (
                "config/database.yml",
                "default: &default\n  adapter: postgresql\n  host: <%= ENV.fetch(\"DB_HOST\") { \"localhost\" } %>\n  port: 5432\n",
            ),
            (
                ".env.example",
                "REDIS_URL=redis://192.168.8.31:6379/0\nSEARCH_HOST=172.17.0.9\nSEARCH_PORT=9200\n",
            ),
            ("Gemfile", "source 'https://rubygems.org'\n"),
        ]);
        let found: Vec<(String, String, u16, Where)> = needs
            .services
            .iter()
            .map(|s| (s.name.clone(), s.host.clone(), s.port, s.place))
            .collect();
        assert!(found.contains(&(
            "PostgreSQL".into(),
            "localhost".into(),
            5432,
            Where::ThisComputer
        )));
        assert!(found.contains(&(
            "Redis".into(),
            "192.168.8.31".into(),
            6379,
            Where::LocalNetwork
        )));
        assert!(found.contains(&(
            "Elasticsearch".into(),
            "172.17.0.9".into(),
            9200,
            Where::ThisComputer
        )));
        assert!(needs.internet);
        assert_eq!(needs.installs, vec!["gems".to_string()]);
        assert_eq!(reach_entries(&needs), vec!["192.168.8.31:6379".to_string()]);
    }

    #[test]
    fn the_setup_says_it_in_one_sentence() {
        let needs = needs_of(&[
            (
                "compose.yaml",
                "services:\n  postgres:\n    image: postgres\n    ports: [\"5432:5432\"]\n  redis:\n    image: redis\n    ports: [\"6379:6379\"]\n",
            ),
            ("Gemfile", "gem 'rails'\n"),
        ]);
        assert_eq!(
            sentence(&needs).unwrap(),
            "It can reach PostgreSQL and Redis on this computer, and the internet to install \
             gems (from compose.yaml and Gemfile)"
        );
        assert_eq!(sentence(&Needs::default()), None);
    }

    #[test]
    fn a_folder_without_what_the_code_needs_is_told_apart() {
        let mut root = Root {
            id: "bots".into(),
            label: "Bots".into(),
            path: "/tmp".into(),
            follow_symlinks: false,
            writable: true,
            commands: true,
            network: false,
            local_services: false,
            reach: Vec::new(),
            bots: Vec::new(),
            read_budget: None,
        };
        let local = needs_of(&[(".env", "DATABASE_URL=postgres://localhost/app\n")]);
        assert_eq!(allowed(&root, &local), cfg!(windows));
        root.local_services = true;
        assert!(allowed(&root, &local));
        let lan = needs_of(&[(".env", "DATABASE_URL=postgres://192.168.8.31:5432/app\n")]);
        assert!(!allowed(&root, &lan));
        root.reach.push("192.168.8.31:5432".into());
        assert!(allowed(&root, &lan));
        let installs = needs_of(&[("Gemfile", "gem 'rails'\n")]);
        assert!(!allowed(&root, &installs));
        root.network = true;
        assert!(allowed(&root, &installs));
    }

    #[test]
    fn a_mirror_is_read_with_git_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let mirror = dir.path().join("mirror.git");
        let git = |args: &[&str], at: &Path| {
            assert!(
                Command::new("git")
                    .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                    .args(args)
                    .current_dir(at)
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "{args:?}"
            );
        };
        std::fs::create_dir_all(work.join("config")).unwrap();
        std::fs::write(
            work.join("config/database.yml"),
            "test:\n  adapter: mysql2\n  host: 127.0.0.1\n",
        )
        .unwrap();
        std::fs::write(work.join("package.json"), r#"{"dependencies":{"x":"1"}}"#).unwrap();
        git(&["init", "-q"], &work);
        git(&["add", "."], &work);
        git(&["commit", "-qm", "x"], &work);
        git(
            &[
                "clone",
                "-q",
                "--bare",
                work.to_str().unwrap(),
                mirror.to_str().unwrap(),
            ],
            dir.path(),
        );
        let needs = from_mirror(&mirror, &chalk());
        assert_eq!(needs.services.len(), 1);
        assert_eq!(needs.services[0].name, "MySQL");
        assert_eq!(needs.services[0].port, 3306);
        assert_eq!(needs.services[0].from, "config/database.yml");
        assert!(needs.internet);
        assert_eq!(needs.installs, vec!["npm packages".to_string()]);
    }
}
