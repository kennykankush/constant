//! Live census — which agent CLI processes are running on this machine RIGHT
//! NOW, and which sessions they hold.
//!
//! Read-only: one `ps` walk plus a best-effort cwd lookup per agent. The
//! census recognizes the actual agent processes and skips the wrappers around
//! them (`dtach` masters/clients, `/bin/sh -c …` launchers, login shells, and
//! Constant's own `host`), so each live conversation is counted exactly once.

use std::process::Command;

use crate::runtime::Runtime;

pub struct LiveAgent {
    pub runtime: Runtime,
    pub pid: i32,
    /// Elapsed time as `ps` renders it (e.g. `02:13:45`, `1-02:34:19`).
    pub up: String,
    /// The session id declared in the process args (`--resume`, `-s`, …),
    /// when one is visible. A fresh launch has none.
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

/// Walk the process table and return every live agent process.
pub fn census() -> Vec<LiveAgent> {
    let Ok(out) = Command::new("ps")
        .args(["-axo", "pid=,etime=,command="])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut agents: Vec<LiveAgent> = text.lines().filter_map(parse_ps_line).collect();
    agents.sort_by(|a, b| {
        a.runtime
            .label()
            .cmp(b.runtime.label())
            .then(a.pid.cmp(&b.pid))
    });
    // An agent stack can surface the same session twice (codex's node launcher
    // AND its vendored native binary). One session id = one live agent; the
    // parent (lowest pid) wins. Session-unknown entries are kept as-is — they
    // may be genuinely distinct fresh launches.
    let mut seen = std::collections::HashSet::new();
    agents.retain(|a| match &a.session_id {
        Some(id) => seen.insert((a.runtime, id.clone())),
        None => true,
    });
    for agent in &mut agents {
        agent.cwd = cwd_of(agent.pid);
    }
    agents
}

fn parse_ps_line(line: &str) -> Option<LiveAgent> {
    let mut it = line.split_whitespace();
    let pid: i32 = it.next()?.parse().ok()?;
    let up = it.next()?.to_string();
    let tokens: Vec<&str> = it.collect();
    if tokens.is_empty() {
        return None;
    }
    let (runtime, args) = classify(&tokens)?;
    let session_id = extract_session_id(runtime, args);
    Some(LiveAgent {
        runtime,
        pid,
        up,
        session_id,
        cwd: None,
    })
}

/// Decide whether a command line IS an agent process (not a wrapper). The
/// executable must be the agent binary itself — possibly behind a `node`/`bun`
/// interpreter (codex runs as `node /opt/homebrew/bin/codex …`). Login-shell
/// markers (a leading `-`) are stripped before matching.
fn classify<'a>(tokens: &'a [&'a str]) -> Option<(Runtime, &'a [&'a str])> {
    let runtime_of = |token: &str| -> Option<Runtime> {
        match basename(token.trim_start_matches('-')) {
            "claude" => Some(Runtime::Claude),
            "codex" => Some(Runtime::Codex),
            "gemini" => Some(Runtime::Gemini),
            "opencode" => Some(Runtime::OpenCode),
            _ => None,
        }
    };

    if let Some(rt) = runtime_of(tokens[0]) {
        return Some((rt, &tokens[1..]));
    }
    // Interpreter-launched agents: `node /path/to/codex resume <id>`.
    if matches!(basename(tokens[0].trim_start_matches('-')), "node" | "bun")
        && tokens.len() > 1
        && let Some(rt) = runtime_of(tokens[1])
    {
        return Some((rt, &tokens[2..]));
    }
    None
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Pull the session id out of the agent's own arguments, per runtime.
fn extract_session_id(runtime: Runtime, args: &[&str]) -> Option<String> {
    let id_after = |flags: &[&str]| -> Option<String> {
        args.windows(2).find_map(|w| {
            (flags.contains(&w[0]) && looks_like_session_id(w[1])).then(|| w[1].to_string())
        })
    };
    match runtime {
        Runtime::Claude => id_after(&["--resume", "-r", "--session-id"]),
        Runtime::Codex => id_after(&["resume"]),
        Runtime::Gemini => id_after(&["--resume"]),
        Runtime::OpenCode => id_after(&["-s", "--session"]),
    }
}

fn looks_like_session_id(s: &str) -> bool {
    s.len() >= 4
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The process's working directory, best-effort (read-only).
#[cfg(target_os = "linux")]
fn cwd_of(pid: i32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.display().to_string())
}

#[cfg(not(target_os = "linux"))]
fn cwd_of(pid: i32) -> Option<String> {
    let out = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix('n').map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(line: &str) -> Option<LiveAgent> {
        parse_ps_line(line)
    }

    #[test]
    fn recognizes_real_agent_processes() {
        let a = agent(
            "4093 03:23:38 /opt/homebrew/bin/claude --dangerously-skip-permissions --resume e49a12d7-df13-40a9-afc1-e5090fcbdfea",
        )
        .expect("claude not recognized");
        assert_eq!(a.runtime, Runtime::Claude);
        assert_eq!(
            a.session_id.as_deref(),
            Some("e49a12d7-df13-40a9-afc1-e5090fcbdfea")
        );

        let a = agent(
            "67339 01-02:33:34 node /opt/homebrew/bin/codex --dangerously-bypass-approvals-and-sandbox resume 019e9a25-91e9-7d53-961b-6b17b461afbf",
        )
        .expect("node-wrapped codex not recognized");
        assert_eq!(a.runtime, Runtime::Codex);
        assert_eq!(
            a.session_id.as_deref(),
            Some("019e9a25-91e9-7d53-961b-6b17b461afbf")
        );

        let a = agent("900 10:00 /usr/local/bin/opencode -s ses_2ad0a27bcffeBnD4VbVSokL3zT")
            .expect("opencode not recognized");
        assert_eq!(a.runtime, Runtime::OpenCode);
        assert_eq!(
            a.session_id.as_deref(),
            Some("ses_2ad0a27bcffeBnD4VbVSokL3zT")
        );

        // Fresh launch: agent recognized, session unknown.
        let a = agent("901 00:05 /opt/homebrew/bin/claude").expect("fresh claude");
        assert_eq!(a.session_id, None);
    }

    #[test]
    fn wrappers_are_not_counted_as_agents() {
        // dtach master holding a claude — the claude INSIDE it is its own
        // process; the wrapper must not double-count.
        assert!(
            agent(
                "4174 01-12:34:07 -/opt/homebrew/bin/dtach -A /x.sock -z -r winch /bin/sh -c /opt/homebrew/bin/claude --resume x1"
            )
            .is_none(),
            "dtach wrapper counted"
        );
        assert!(
            agent("500 01:00 /bin/sh -c /opt/homebrew/bin/claude --resume abcd1234").is_none(),
            "sh wrapper counted"
        );
        assert!(
            agent("501 01:00 /usr/bin/login -flp user /bin/bash -c exec claude").is_none(),
            "login wrapper counted"
        );
        // Constant's own host mentions runtimes as ARGS, not as the binary.
        assert!(
            agent("502 01:00 constant host codex").is_none(),
            "constant host counted as an agent"
        );
        assert!(
            agent("503 01:00 grep claude").is_none(),
            "grep counted as an agent"
        );
    }

    #[test]
    fn session_id_extraction_is_conservative() {
        // `claude -r` with no id (interactive picker): no id extracted.
        let a = agent("904 00:01 /opt/homebrew/bin/claude -r").unwrap();
        assert_eq!(a.session_id, None);
        // A following FLAG is never mistaken for an id.
        let a = agent("905 00:01 /opt/homebrew/bin/claude --resume --verbose").unwrap();
        assert_eq!(a.session_id, None);
    }
}
