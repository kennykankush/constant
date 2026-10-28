//! `constant integration`: install the built-in agent guide (`constant --skill`)
//! as a skill in each runtime's own skills folder, so an agent running there
//! knows how to use Constant without being told.
//!
//! Where each runtime loads skills from (verified against the real binaries,
//! 2026-10-28: claude 2.1.289, codex 0.165.0, opencode 2.0.28):
//! - claude:   `$CLAUDE_CONFIG_DIR/skills` (default `~/.claude/skills`)
//! - codex:    `$CODEX_HOME/skills` (default `~/.codex/skills`); it also reads
//!   `~/.agents/skills`, but not `~/.claude/skills`
//! - opencode: `$XDG_CONFIG_HOME/opencode/skills` (default
//!   `~/.config/opencode/skills`); it also reads `~/.claude/skills` and
//!   `~/.agents/skills`, keeps one skill per name, and its own folder wins
//!
//! Only a file Constant wrote is ever replaced or removed: an existing
//! `constant/SKILL.md` that is not the Constant guide is left alone.

use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::alembic::formats;

pub const SKILL: &str = include_str!("../skills/constant/SKILL.md");

const RUNTIMES: [&str; 3] = ["claude", "codex", "opencode"];

/// The installed guide's path for a runtime.
fn skill_path(runtime: &str) -> Result<PathBuf> {
    let skills = match runtime {
        "claude" => formats::claude_root()?.join("skills"),
        "codex" => formats::codex_root()?.join("skills"),
        "opencode" => opencode_config_root()?.join("skills"),
        other => bail!("unknown runtime: {other} (use claude, codex, opencode, or all)"),
    };
    Ok(skills.join("constant").join("SKILL.md"))
}

/// OpenCode's global config folder; respects `XDG_CONFIG_HOME` like opencode.
fn opencode_config_root() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(xdg).join("opencode"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config").join("opencode"))
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    Missing,
    Current,
    /// Constant's guide, from another version of the binary.
    Outdated,
    /// Something else lives at `constant/SKILL.md`; never touched.
    Foreign,
}

impl State {
    fn label(&self) -> &'static str {
        match self {
            State::Missing => "not installed",
            State::Current => "installed",
            State::Outdated => "installed, outdated (run install again)",
            State::Foreign => "a different skill named constant is there (left alone)",
        }
    }
}

fn is_constant_guide(text: &str) -> bool {
    text.starts_with("---\nname: constant\n")
}

fn state_of(path: &Path) -> State {
    match fs::read_to_string(path) {
        Err(_) if !path.exists() => State::Missing,
        Err(_) => State::Foreign,
        Ok(text) if text == SKILL => State::Current,
        Ok(text) if is_constant_guide(&text) => State::Outdated,
        Ok(_) => State::Foreign,
    }
}

/// Atomic like every other Constant write: tmp sibling, fsync, rename.
fn write_guide(path: &Path) -> Result<()> {
    let dir = path.parent().context("skill path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let tmp = formats::tmp_sibling(path);
    let mut guard = formats::TmpCleanup::new(&tmp);
    let mut file =
        fs::File::create(&tmp).with_context(|| format!("could not write {}", tmp.display()))?;
    file.write_all(SKILL.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, path).with_context(|| format!("could not write {}", path.display()))?;
    guard.keep();
    Ok(())
}

fn targets(arg: Option<&str>) -> Result<Vec<&'static str>> {
    match arg {
        Some("all") => Ok(RUNTIMES.to_vec()),
        Some(rt) => match RUNTIMES.iter().find(|r| **r == rt) {
            Some(r) => Ok(vec![*r]),
            None => bail!("unknown runtime: {rt} (use claude, codex, opencode, or all)"),
        },
        None => bail!("name a runtime: claude, codex, opencode, or all"),
    }
}

/// `constant integration [install|uninstall] [claude|codex|opencode|all] [--json]`
pub fn run(rest: &[String]) -> Result<()> {
    let json = rest.iter().any(|a| a == "--json");
    let args: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
        .collect();

    match args.first().copied() {
        None | Some("status") => status(json),
        Some("install") => {
            let mut rows = Vec::new();
            for rt in targets(args.get(1).copied())? {
                let path = skill_path(rt)?;
                let outcome = match state_of(&path) {
                    State::Foreign => "skipped: a different skill named constant is there",
                    State::Current => "already current",
                    State::Missing | State::Outdated => {
                        write_guide(&path)?;
                        "installed"
                    }
                };
                rows.push((rt, path, outcome));
            }
            report(&rows, json)
        }
        Some("uninstall") => {
            let mut rows = Vec::new();
            for rt in targets(args.get(1).copied())? {
                let path = skill_path(rt)?;
                let outcome = match state_of(&path) {
                    State::Missing => "not installed",
                    State::Foreign => "skipped: not Constant's guide",
                    State::Current | State::Outdated => {
                        fs::remove_file(&path)
                            .with_context(|| format!("could not remove {}", path.display()))?;
                        // Only an empty folder; never anything else in it.
                        if let Some(dir) = path.parent() {
                            let _ = fs::remove_dir(dir);
                        }
                        "removed"
                    }
                };
                rows.push((rt, path, outcome));
            }
            report(&rows, json)
        }
        Some(other) => bail!(
            "unknown integration command: {other} (use install, uninstall, or no argument for status)"
        ),
    }
}

fn status(json: bool) -> Result<()> {
    let mut rows = Vec::new();
    for rt in RUNTIMES {
        let path = skill_path(rt)?;
        rows.push((rt, path.clone(), state_of(&path).label()));
    }
    if !json {
        println!("Constant's agent guide, per runtime:\n");
    }
    report(&rows, json)?;
    if !json {
        println!("\n  constant integration install claude|codex|opencode|all");
        println!("  constant integration uninstall claude|codex|opencode|all");
    }
    Ok(())
}

fn report(rows: &[(&str, PathBuf, &str)], json: bool) -> Result<()> {
    if json {
        let out: Vec<_> = rows
            .iter()
            .map(|(rt, path, state)| {
                serde_json::json!({
                    "runtime": rt,
                    "path": path.display().to_string(),
                    "state": state,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        for (rt, path, state) in rows {
            println!("  {rt:<9} {state}\n            {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_is_recognized_and_others_are_not() {
        assert!(is_constant_guide(SKILL));
        assert!(!is_constant_guide("---\nname: constant-tools\n---\n"));
        assert!(!is_constant_guide("# my own constant notes\n"));
    }

    #[test]
    fn targets_expand_all_and_reject_unknowns() {
        assert_eq!(targets(Some("all")).unwrap(), RUNTIMES.to_vec());
        assert_eq!(targets(Some("codex")).unwrap(), vec!["codex"]);
        assert!(targets(Some("gemini")).is_err());
        assert!(targets(None).is_err());
    }
}
