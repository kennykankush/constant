//! Runtime definitions: which agent CLIs Constant can host, and how to launch them.

use anyhow::{bail, Result};
use portable_pty::CommandBuilder;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Runtime {
    Codex,
    Claude,
}

impl Runtime {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.trim().to_lowercase().as_str() {
            "codex" | "x" => Runtime::Codex,
            "claude" | "c" => Runtime::Claude,
            other => bail!("unknown runtime: {other} (use codex|claude)"),
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Runtime::Codex => "codex",
            Runtime::Claude => "claude",
        }
    }

    fn bin(self) -> &'static str {
        match self {
            Runtime::Codex => "codex",
            Runtime::Claude => "claude",
        }
    }

    /// Build a fresh interactive launch command for this runtime.
    ///
    /// portable-pty's CommandBuilder does NOT inherit the parent environment by
    /// default, so we copy it explicitly — PATH, HOME, TERM all matter for the
    /// child TUI to behave natively.
    pub fn fresh_command(self) -> CommandBuilder {
        self.command(&[])
    }

    /// Build a command that resumes an existing native session by id
    /// (`claude -r <id>` / `codex resume <id>`).
    pub fn resume_command(self, session_id: &str) -> CommandBuilder {
        match self {
            Runtime::Codex => self.command(&["resume", session_id]),
            Runtime::Claude => self.command(&["-r", session_id]),
        }
    }

    fn command(self, args: &[&str]) -> CommandBuilder {
        let mut cmd = CommandBuilder::new(self.bin());
        for arg in args {
            cmd.arg(arg);
        }
        for (key, value) in std::env::vars() {
            cmd.env(key, value);
        }
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        cmd
    }
}
