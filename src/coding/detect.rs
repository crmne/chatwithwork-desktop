//! What a checkout needs to run its tests: its toolchain, how to install its
//! dependencies, and its test command, read from its files.

use std::path::Path;

/// One thing the checkout uses, and how to set it up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    /// What it is, for the person: "Ruby gems (Gemfile)".
    pub what: String,
    /// The program it needs on this computer.
    pub program: &'static str,
    /// The command that installs it, run in the checkout as the person.
    pub install: Option<String>,
}

fn has(dir: &Path, name: &str) -> bool {
    dir.join(name).exists()
}

/// What `checkout` needs, in the order to set it up: the toolchain first.
pub fn needs(checkout: &Path) -> Vec<Need> {
    let mut needs = Vec::new();
    if has(checkout, "mise.toml") || has(checkout, ".mise.toml") || has(checkout, ".tool-versions")
    {
        needs.push(Need {
            what: "the toolchain mise installs (mise.toml or .tool-versions)".into(),
            program: "mise",
            install: Some("mise trust --yes && mise install".into()),
        });
    }
    if has(checkout, "Gemfile") {
        needs.push(Need {
            what: "Ruby gems (Gemfile)".into(),
            program: "bundle",
            install: Some("bundle install".into()),
        });
    }
    if has(checkout, "package.json") {
        let (program, install) = if has(checkout, "pnpm-lock.yaml") {
            ("pnpm", "pnpm install --frozen-lockfile")
        } else if has(checkout, "yarn.lock") {
            ("yarn", "yarn install --immutable")
        } else if has(checkout, "bun.lock") || has(checkout, "bun.lockb") {
            ("bun", "bun install --frozen-lockfile")
        } else if has(checkout, "package-lock.json") {
            ("npm", "npm ci")
        } else {
            ("npm", "npm install")
        };
        needs.push(Need {
            what: "JavaScript packages (package.json)".into(),
            program,
            install: Some(install.into()),
        });
    }
    if has(checkout, "Cargo.toml") {
        needs.push(Need {
            what: "Rust crates (Cargo.toml)".into(),
            program: "cargo",
            install: Some("cargo fetch".into()),
        });
    }
    if has(checkout, "pyproject.toml") || has(checkout, "requirements.txt") {
        let need = if has(checkout, "uv.lock") {
            Need {
                what: "Python packages (uv.lock)".into(),
                program: "uv",
                install: Some("uv sync".into()),
            }
        } else if has(checkout, "poetry.lock") {
            Need {
                what: "Python packages (poetry.lock)".into(),
                program: "poetry",
                install: Some("poetry install".into()),
            }
        } else {
            Need {
                what: "Python (pyproject.toml or requirements.txt)".into(),
                program: "python3",
                install: None,
            }
        };
        needs.push(need);
    }
    if has(checkout, "go.mod") {
        needs.push(Need {
            what: "Go modules (go.mod)".into(),
            program: "go",
            install: Some("go mod download".into()),
        });
    }
    needs
}

/// The command that most likely runs the checkout's tests.
pub fn test_command(checkout: &Path) -> Option<String> {
    let read = |name: &str| std::fs::read_to_string(checkout.join(name)).unwrap_or_default();
    let mise = read("mise.toml") + &read(".mise.toml");
    if mise.contains("[tasks.test]") || mise.contains("[tasks.\"test\"]") {
        return Some("mise run test".into());
    }
    if has(checkout, "bin/rails") {
        return Some("bin/rails test".into());
    }
    if has(checkout, "Gemfile") && has(checkout, "Rakefile") {
        return Some("bundle exec rake test".into());
    }
    if has(checkout, "package.json") {
        let package: serde_json::Value =
            serde_json::from_str(&read("package.json")).unwrap_or_default();
        if package["scripts"]["test"].is_string() {
            let runner = if has(checkout, "pnpm-lock.yaml") {
                "pnpm"
            } else if has(checkout, "yarn.lock") {
                "yarn"
            } else if has(checkout, "bun.lock") || has(checkout, "bun.lockb") {
                "bun run"
            } else {
                "npm"
            };
            return Some(format!("{runner} test"));
        }
    }
    if has(checkout, "Cargo.toml") {
        return Some("cargo test".into());
    }
    if has(checkout, "go.mod") {
        return Some("go test ./...".into());
    }
    if has(checkout, "pyproject.toml") || has(checkout, "pytest.ini") {
        return Some(if has(checkout, "uv.lock") {
            "uv run pytest".into()
        } else {
            "python3 -m pytest".into()
        });
    }
    if read("Makefile").lines().any(|l| l.starts_with("test:")) {
        return Some("make test".into());
    }
    None
}
