//! Live census — which agent CLI processes are running on this machine RIGHT
//! NOW, and which sessions they hold.
//!
//! Read-only: one `ps` walk plus a best-effort cwd lookup per agent. The
//! census recognizes the actual agent processes and skips the wrappers around
//! them (`dtach` masters/clients, `/bin/sh -c …` launchers, login shells, and
//! Constant's own `host`), so each live conversation is counted exactly once.
//! It also skips what is not a conversation at all: an agent's background
//! services (`codex app-server`, …), a launcher's own native child (codex runs
//! as `node …/codex` → vendored binary), and agents living inside a desktop app
//! bundle (the ChatGPT app's codex runtimes), which Constant cannot host.

use std::process::Command;

use crate::runtime::Runtime;

pub struct LiveAgent {
    pub runtime: Runtime,
    pub pid: i32,
    /// Parent pid — a launcher's native child is folded into the launcher.
    pub ppid: i32,
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
        .args(["-axo", "pid=,ppid=,etime=,command="])
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
    fold_agent_stacks(&mut agents);
    for agent in &mut agents {
        agent.cwd = cwd_of(agent.pid);
    }
    agents
}

/// One conversation, one row. An agent stack surfaces one conversation as two
/// processes: codex's node launcher AND its vendored native binary (same args,
/// so the same session id — or none, for a fresh launch). The launcher is the
/// agent; a child of a same-runtime agent is its engine. Then, for stacks that
/// re-parent: one session id = one live agent, the lowest pid wins (`agents`
/// arrive pid-sorted per runtime). Session-unknown entries are otherwise kept —
/// they may be genuinely distinct fresh launches.
fn fold_agent_stacks(agents: &mut Vec<LiveAgent>) {
    let parents: std::collections::HashSet<(Runtime, i32)> =
        agents.iter().map(|a| (a.runtime, a.pid)).collect();
    agents.retain(|a| !parents.contains(&(a.runtime, a.ppid)));
    let mut seen = std::collections::HashSet::new();
    agents.retain(|a| match &a.session_id {
        Some(id) => seen.insert((a.runtime, id.clone())),
        None => true,
    });
}

fn parse_ps_line(line: &str) -> Option<LiveAgent> {
    let mut it = line.split_whitespace();
    let pid: i32 = it.next()?.parse().ok()?;
    let ppid: i32 = it.next()?.parse().ok()?;
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
        ppid,
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

    let (exe, rt, args) = if let Some(rt) = runtime_of(tokens[0]) {
        (tokens[0], rt, &tokens[1..])
    } else if matches!(basename(tokens[0].trim_start_matches('-')), "node" | "bun")
        && tokens.len() > 1
        && let Some(rt) = runtime_of(tokens[1])
    {
        // Interpreter-launched agents: `node /path/to/codex resume <id>`.
        (tokens[1], rt, &tokens[2..])
    } else {
        return None;
    };
    // A desktop app's embedded agent (`/Applications/ChatGPT.app/Contents/…/
    // codex`) is the app's own runtime, not a terminal conversation.
    if exe.contains(".app/Contents/") {
        return None;
    }
    // Background services an agent CLI runs for itself are not conversations.
    if args.iter().any(|a| SERVICE_SUBCOMMANDS.contains(a)) {
        return None;
    }
    Some((rt, args))
}

/// Agent-CLI subcommands that run a server or daemon rather than a session.
const SERVICE_SUBCOMMANDS: &[&str] = &["app-server", "exec-server", "mcp-server", "remote-control"];

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
            "4093 1 03:23:38 /opt/homebrew/bin/claude --dangerously-skip-permissions --resume e49a12d7-df13-40a9-afc1-e5090fcbdfea",
        )
        .expect("claude not recognized");
        assert_eq!(a.runtime, Runtime::Claude);
        assert_eq!(
            a.session_id.as_deref(),
            Some("e49a12d7-df13-40a9-afc1-e5090fcbdfea")
        );

        let a = agent(
            "67339 1 01-02:33:34 node /opt/homebrew/bin/codex --dangerously-bypass-approvals-and-sandbox resume 019e9a25-91e9-7d53-961b-6b17b461afbf",
        )
        .expect("node-wrapped codex not recognized");
        assert_eq!(a.runtime, Runtime::Codex);
        assert_eq!(
            a.session_id.as_deref(),
            Some("019e9a25-91e9-7d53-961b-6b17b461afbf")
        );

        let a = agent("900 1 10:00 /usr/local/bin/opencode -s ses_2ad0a27bcffeBnD4VbVSokL3zT")
            .expect("opencode not recognized");
        assert_eq!(a.runtime, Runtime::OpenCode);
        assert_eq!(
            a.session_id.as_deref(),
            Some("ses_2ad0a27bcffeBnD4VbVSokL3zT")
        );

        // Fresh launch: agent recognized, session unknown.
        let a = agent("901 1 00:05 /opt/homebrew/bin/claude").expect("fresh claude");
        assert_eq!(a.session_id, None);
    }

    #[test]
    fn wrappers_are_not_counted_as_agents() {
        // dtach master holding a claude — the claude INSIDE it is its own
        // process; the wrapper must not double-count.
        assert!(
            agent(
                "4174 1 01-12:34:07 -/opt/homebrew/bin/dtach -A /x.sock -z -r winch /bin/sh -c /opt/homebrew/bin/claude --resume x1"
            )
            .is_none(),
            "dtach wrapper counted"
        );
        assert!(
            agent("500 1 01:00 /bin/sh -c /opt/homebrew/bin/claude --resume abcd1234").is_none(),
            "sh wrapper counted"
        );
        assert!(
            agent("501 1 01:00 /usr/bin/login -flp user /bin/bash -c exec claude").is_none(),
            "login wrapper counted"
        );
        // Constant's own host mentions runtimes as ARGS, not as the binary.
        assert!(
            agent("502 1 01:00 constant host codex").is_none(),
            "constant host counted as an agent"
        );
        assert!(
            agent("503 1 01:00 grep claude").is_none(),
            "grep counted as an agent"
        );
    }

    #[test]
    fn services_and_app_bundles_are_not_conversations() {
        assert!(
            agent("77434 1 06-03:03:02 /Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex app-server --listen stdio://").is_none(),
            "desktop app's codex counted"
        );
        assert!(
            agent("7674 1 04:20 node /opt/homebrew/bin/codex app-server").is_none(),
            "codex app-server daemon counted"
        );
        assert!(
            agent("7675 1 04:20 /opt/homebrew/bin/codex exec-server").is_none(),
            "codex exec-server counted"
        );
    }

    #[test]
    fn a_launchers_native_child_is_not_a_second_agent() {
        // `node …/codex` (pid 34355) spawns the vendored binary (pid 34356) with
        // the same args; both classify as codex, only the launcher survives.
        let lines = [
            "34355 1 03-11:02:17 node /opt/homebrew/bin/codex --enable hooks -C /tmp/x",
            "34356 34355 03-11:02:17 /opt/homebrew/lib/node_modules/@openai/codex/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex --enable hooks -C /tmp/x",
            "34400 1 00:10 node /opt/homebrew/bin/codex",
        ];
        let mut agents: Vec<LiveAgent> = lines.iter().filter_map(|l| parse_ps_line(l)).collect();
        assert_eq!(agents.len(), 3, "all three classify as codex before folding");
        fold_agent_stacks(&mut agents);
        let pids: Vec<i32> = agents.iter().map(|a| a.pid).collect();
        assert_eq!(pids, vec![34355, 34400], "native child folded into its launcher");
    }

    #[test]
    fn session_id_extraction_is_conservative() {
        // `claude -r` with no id (interactive picker): no id extracted.
        let a = agent("904 1 00:01 /opt/homebrew/bin/claude -r").unwrap();
        assert_eq!(a.session_id, None);
        // A following FLAG is never mistaken for an id.
        let a = agent("905 1 00:01 /opt/homebrew/bin/claude --resume --verbose").unwrap();
        assert_eq!(a.session_id, None);
    }
}
