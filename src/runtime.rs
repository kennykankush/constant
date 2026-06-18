//! Runtime definitions: which agent CLIs Constant can host, and how to launch them.

use anyhow::{Result, bail};
use portable_pty::CommandBuilder;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Runtime {
    Codex,
    Claude,
    Gemini,
    OpenCode,
}

impl Runtime {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.trim().to_lowercase().as_str() {
            "codex" | "x" => Runtime::Codex,
            "claude" | "c" => Runtime::Claude,
            "gemini" | "g" => Runtime::Gemini,
            "opencode" | "o" => Runtime::OpenCode,
            other => bail!("unknown runtime: {other} (use codex|claude|gemini|opencode)"),
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Runtime::Codex => "codex",
            Runtime::Claude => "claude",
            Runtime::Gemini => "gemini",
            Runtime::OpenCode => "opencode",
        }
    }

    fn bin(self) -> &'static str {
        self.label()
    }

    /// Build a fresh interactive launch command for this runtime. When
    /// `session_id` is given and the runtime supports declaring one (claude's
    /// `--session-id`), the child's identity is KNOWN to the harness from
    /// birth instead of inferred from the filesystem later.
    ///
    /// portable-pty's CommandBuilder does NOT inherit the parent environment by
    /// default, so we copy it explicitly — PATH, HOME, TERM all matter for the
    /// child TUI to behave natively.
    pub fn fresh_command(self, session_id: Option<&str>, yolo: bool) -> CommandBuilder {
        let mut args: Vec<&str> = match (self, session_id) {
            (Runtime::Claude, Some(id)) => vec!["--session-id", id],
            _ => vec![],
        };
        if yolo && let Some(flag) = self.yolo_flag() {
            args.push(flag);
        }
        self.command(&args)
    }

    /// Build a command that resumes an existing native session by id
    /// (`claude -r <id>` / `codex resume <id>`).
    pub fn resume_command(self, session_id: &str, yolo: bool) -> CommandBuilder {
        let mut args: Vec<&str> = match self {
            Runtime::Codex => vec!["resume", session_id],
            Runtime::Claude => vec!["-r", session_id],
            Runtime::Gemini => vec!["--resume", session_id],
            Runtime::OpenCode => vec!["-s", session_id],
        };
        if yolo && let Some(flag) = self.yolo_flag() {
            args.push(flag);
        }
        self.command(&args)
    }

    /// The runtime's "run without sandbox / approval prompts" flag, injected
    /// only under `--yolo` (explicit opt-in; EXTREMELY DANGEROUS). opencode's
    /// permission model is config-based (no CLI bypass); gemini is carry-
    /// source-only and never spawned.
    fn yolo_flag(self) -> Option<&'static str> {
        match self {
            Runtime::Codex => Some("--dangerously-bypass-approvals-and-sandbox"),
            Runtime::Claude => Some("--dangerously-skip-permissions"),
            Runtime::OpenCode | Runtime::Gemini => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(cmd: CommandBuilder) -> Vec<String> {
        cmd.get_argv()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn yolo_injects_the_per_runtime_bypass_flag_only_when_asked() {
        // claude → --dangerously-skip-permissions, resume AND fresh, only under yolo
        assert!(
            argv(Runtime::Claude.resume_command("id", true))
                .contains(&"--dangerously-skip-permissions".into())
        );
        assert!(
            argv(Runtime::Claude.fresh_command(Some("id"), true))
                .contains(&"--dangerously-skip-permissions".into())
        );
        assert!(
            !argv(Runtime::Claude.resume_command("id", false))
                .iter()
                .any(|a| a.contains("dangerous"))
        );
        // codex → the bypass-approvals-and-sandbox flag
        assert!(
            argv(Runtime::Codex.resume_command("id", true))
                .contains(&"--dangerously-bypass-approvals-and-sandbox".into())
        );
        // opencode has no CLI bypass — never injected, even under yolo
        assert!(
            !argv(Runtime::OpenCode.resume_command("id", true))
                .iter()
                .any(|a| a.contains("dangerous"))
        );
    }
}
