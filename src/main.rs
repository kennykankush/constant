//! Constant — one conversation, any agent runtime.
//!
//! `constant host [runtime]` boots an agent CLI inside a Constant-owned PTY so
//! you can switch the active runtime live (tmux-style prefix key) without losing
//! the conversation.

mod alembic;
mod host;
mod runtime;

use anyhow::{bail, Context, Result};
use runtime::Runtime;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("host") => run_host(&args[1..]),
        Some("distill") => run_distill(&args[1..]),
        Some("keys") => host::debug_keys(),
        Some("-h") | Some("--help") | None => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("constant: unknown command: {other}\n");
            print_help();
            std::process::exit(1);
        }
    }
}

fn run_host(rest: &[String]) -> Result<()> {
    let mut runtime_str = "codex".to_string();
    let mut prefix_str = std::env::var("CONSTANT_PREFIX").unwrap_or_else(|_| "C-b".to_string());

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--prefix" | "-p" => {
                prefix_str = rest
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("--prefix needs a value, e.g. --prefix C-t"))?;
                i += 2;
            }
            s if !s.starts_with('-') => {
                runtime_str = s.to_string();
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let (prefix_byte, prefix_label) = host::parse_prefix(&prefix_str)?;
    host::run(Runtime::parse(&runtime_str)?, prefix_byte, prefix_label)
}

fn run_distill(rest: &[String]) -> Result<()> {
    let mut from: Option<String> = None;
    let mut to: Option<String> = None;
    let mut session: Option<PathBuf> = None;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--from" => {
                from = rest.get(i + 1).cloned();
                i += 2;
            }
            "--to" => {
                to = rest.get(i + 1).cloned();
                i += 2;
            }
            "--session" => {
                session = rest.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let to = Runtime::parse(&to.context("distill requires --to codex|claude")?)?;
    let (id, cwd) = match session {
        Some(path) => {
            let (id, _written, cwd) = alembic::distill_path(&path, to, None)?;
            (id, cwd)
        }
        None => {
            let from = Runtime::parse(&from.context("distill requires --from or --session")?)?;
            let here = std::env::current_dir().ok();
            alembic::distill(from, to, here.as_deref())?
        }
    };

    println!("distilled → {} session {id}", to.label());
    if let Some(cwd) = cwd {
        println!("cwd: {}", cwd.display());
    }
    match to {
        Runtime::Claude => println!("resume with: claude -r {id}"),
        Runtime::Codex => println!("resume with: codex resume {id}"),
    }
    Ok(())
}

fn print_help() {
    println!(
        r#"Constant — one conversation, any agent runtime.

USAGE:
  constant host [codex|claude] [--prefix C-t]
        Host an agent CLI in a Constant PTY (default runtime: codex, prefix: Ctrl-B)

PREFIX KEY:
  Default is Ctrl-B. If you run inside tmux (which also uses Ctrl-B), pick another:
      constant host codex --prefix C-t
      CONSTANT_PREFIX=C-g constant host codex

INSIDE A HOSTED SESSION (press the prefix, then):
  c              switch to claude
  x              switch to codex
  :              open the command line (e.g. `switch claude`, `quit`)
  d              detach / quit the harness
  <prefix> again send a literal prefix key to the child
"#
    );
}
