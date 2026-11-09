//! `constant ask`: consult another agent runtime from an existing conversation.
//!
//! This is the first agent-native continuation primitive that completes the
//! loop without a terminal supervisor: Constant forks the current conversation
//! into a new target projection, runs that runtime non-interactively, and
//! returns the answer to the calling agent.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::runtime::Runtime;

const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DIAGNOSTIC_CAP_CHARS: usize = 16_384;

#[derive(Debug)]
enum Source {
    Session(String),
    From(Runtime),
}

#[derive(Debug)]
struct Options {
    to: Runtime,
    source: Source,
    question: String,
    json: bool,
    with_tools: bool,
    timeout_secs: u64,
    dry_run: bool,
}

struct Answer {
    carry: Value,
    answer: String,
    elapsed_ms: u128,
    question_chars: usize,
}

struct Captured {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// `constant ask --to codex|claude (--from RT | --session PATH_OR_ID)
/// [--question TEXT | TEXT... | --question -] [--json] [--with-tools]
/// [--timeout SECS] [--dry-run]`
pub fn run(rest: &[String]) -> Result<()> {
    if rest
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        print_help();
        return Ok(());
    }
    let json_requested = rest.iter().any(|arg| arg == "--json");
    let opts = match parse(rest) {
        Ok(opts) => opts,
        Err(error) if json_requested => fail_json(format!("{error:#}")),
        Err(error) => return Err(error),
    };
    if opts.dry_run {
        return print_dry_run(&opts);
    }

    match answer(&opts) {
        Ok(result) => {
            if opts.json {
                println!("{}", success_json(&opts, &result));
            } else {
                println!(
                    "{} answered from {} (session {})",
                    opts.to.label(),
                    result
                        .carry
                        .get("handle")
                        .and_then(Value::as_str)
                        .unwrap_or("the carried conversation"),
                    result
                        .carry
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                );
                println!();
                println!("{}", term_safe_block(&result.answer));
                if let Some(resume) = result.carry.get("resume").and_then(Value::as_str) {
                    println!("\nresume with: {}", term_safe_line(resume));
                }
            }
            Ok(())
        }
        Err(error) if opts.json => {
            fail_json(format!("{error:#}"));
        }
        Err(error) => Err(error),
    }
}

fn fail_json(error: String) -> ! {
    let out = json!({
        "ok": false,
        "error": error,
    });
    println!("{out}");
    let _ = std::io::stdout().flush();
    std::process::exit(1);
}

fn print_help() {
    println!(
        "\
constant ask \u{2014} consult another agent runtime from this conversation

USAGE
  constant ask --to codex|claude (--from RT | --session PATH_OR_ID)
               (--question TEXT | TEXT... | --question -)
               [--json] [--with-tools] [--timeout SECS] [--dry-run]

WHAT IT DOES
  Forks the selected conversation into a NEW target projection, runs that
  runtime headlessly on the question, and returns its answer. The source
  conversation and any existing target projection are not modified.

  The question travels over stdin, not argv. `--question -` reads it from
  stdin. Only Codex and Claude Code are verified ask targets today.

  The target runs with that runtime's own permissions and sandbox settings,
  and may inspect or modify the workspace accordingly.

EXAMPLES
  constant ask --to claude --session \"$CODEX_THREAD_ID\" \\
    --question \"Review this plan for hidden failure modes.\" --json

  constant ask --to codex --session \"$CLAUDE_CODE_SESSION_ID\" \\
    --question - < question.txt --json
"
    );
}

fn parse(rest: &[String]) -> Result<Options> {
    let mut to: Option<Runtime> = None;
    let mut source: Option<Source> = None;
    let mut question: Option<String> = None;
    let mut question_words = Vec::new();
    let mut json = false;
    let mut with_tools = false;
    let mut timeout_secs = DEFAULT_TIMEOUT_SECS;
    let mut dry_run = false;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--to" => {
                to = Some(Runtime::parse(&flag_value(rest, i, "--to")?)?);
                i += 2;
            }
            "--session" => {
                if source.is_some() {
                    bail!("--session cannot be combined with --from");
                }
                source = Some(Source::Session(flag_value(rest, i, "--session")?));
                i += 2;
            }
            "--from" => {
                if source.is_some() {
                    bail!("--from cannot be combined with --session");
                }
                source = Some(Source::From(Runtime::parse(&flag_value(
                    rest, i, "--from",
                )?)?));
                i += 2;
            }
            "--question" => {
                question = Some(flag_value(rest, i, "--question")?);
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "--with-tools" => {
                with_tools = true;
                i += 1;
            }
            "--timeout" => {
                let raw = flag_value(rest, i, "--timeout")?;
                timeout_secs = raw
                    .parse()
                    .with_context(|| format!("--timeout must be a positive integer, got {raw}"))?;
                if timeout_secs == 0 {
                    bail!("--timeout must be greater than zero");
                }
                i += 2;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            "--" => {
                i += 1;
            }
            other if other.starts_with('-') => bail!("unknown flag: {other}"),
            other => {
                question_words.push(other.to_string());
                i += 1;
            }
        }
    }

    let to = to.context("ask requires --to codex|claude")?;
    if !matches!(to, Runtime::Codex | Runtime::Claude) {
        bail!(
            "ask currently supports --to codex or --to claude; {} is not a verified headless target",
            to.label()
        );
    }
    let source = source.context("ask requires --from RT or --session PATH_OR_ID")?;
    let mut question = match (question, question_words.is_empty()) {
        (Some(q), true) => q,
        (None, false) => question_words.join(" "),
        (Some(_), false) => bail!("use either --question TEXT or trailing question text, not both"),
        (None, true) => bail!("ask requires a question (--question TEXT or --question -)"),
    };
    if question.trim() == "-" {
        let mut stdin = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin)
            .context("could not read the question from stdin")?;
        question = stdin;
    }
    let question = question.trim().to_string();
    if question.is_empty() {
        bail!("ask question is empty");
    }

    Ok(Options {
        to,
        source,
        question,
        json,
        with_tools,
        timeout_secs,
        dry_run,
    })
}

fn flag_value(rest: &[String], i: usize, flag: &str) -> Result<String> {
    match rest.get(i + 1) {
        Some(v) if !v.starts_with("--") => Ok(v.clone()),
        _ => bail!("{flag} needs a value"),
    }
}

fn carry_args(opts: &Options) -> Vec<String> {
    let mut args = vec![
        "carry".to_string(),
        "--to".to_string(),
        opts.to.label().to_string(),
        "--new".to_string(),
        "--json".to_string(),
    ];
    if opts.with_tools {
        args.push("--with-tools".to_string());
    }
    match &opts.source {
        Source::Session(session) => {
            args.push("--session".to_string());
            args.push(session.clone());
        }
        Source::From(runtime) => {
            args.push("--from".to_string());
            args.push(runtime.label().to_string());
        }
    }
    args
}

fn print_dry_run(opts: &Options) -> Result<()> {
    let args = carry_args(opts);
    let resume = opts.to.ask_args("<target-session>")?;
    if opts.json {
        println!(
            "{}",
            json!({
                "dry_run": true,
                "to": opts.to.label(),
                "carry": args,
                "target_argv": resume,
                "fork": true,
                "question_chars": opts.question.chars().count(),
                "timeout_secs": opts.timeout_secs,
            })
        );
    } else {
        println!(
            "dry-run: would fork the conversation to {}, then run:\n  {} {}",
            opts.to.label(),
            opts.to.label(),
            shell_words(&resume)
        );
    }
    Ok(())
}

fn answer(opts: &Options) -> Result<Answer> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(opts.timeout_secs);
    let carry = carry(opts, deadline)?;
    let id = carry
        .get("id")
        .and_then(Value::as_str)
        .context("carry result did not include the target session id")?;

    let mut cmd = opts.to.ask_command(id)?;
    if let Some(cwd) = carry.get("cwd").and_then(Value::as_str)
        && std::path::Path::new(cwd).is_dir()
    {
        cmd.current_dir(cwd);
    }
    let mut question = opts.question.clone();
    question.push('\n');
    let target = run_captured(cmd, Some(question), deadline, opts.to.label())?;
    let answer = target.stdout.trim().to_string();
    let diagnostics = clip(target.stderr.trim(), DIAGNOSTIC_CAP_CHARS);

    if target.timed_out {
        bail!(
            "{} timed out after {}s{}",
            opts.to.label(),
            opts.timeout_secs,
            diagnostic_suffix(&diagnostics)
        );
    }
    if !target.status.success() {
        bail!(
            "{} exited with {}{}{}",
            opts.to.label(),
            target.status,
            if answer.is_empty() { "" } else { "\nstdout: " },
            if answer.is_empty() {
                diagnostic_suffix(&diagnostics)
            } else {
                format!("{answer}{}", diagnostic_suffix(&diagnostics))
            }
        );
    }
    if answer.is_empty() {
        bail!(
            "{} exited successfully but returned no answer{}",
            opts.to.label(),
            diagnostic_suffix(&diagnostics)
        );
    }

    Ok(Answer {
        carry,
        answer,
        elapsed_ms: started.elapsed().as_millis(),
        question_chars: opts.question.chars().count(),
    })
}

fn carry(opts: &Options, deadline: Instant) -> Result<Value> {
    let exe = std::env::current_exe().context("could not locate the Constant binary")?;
    let args = carry_args(opts);
    let mut cmd = Command::new(&exe);
    cmd.args(&args);
    let output = run_captured(cmd, None, deadline, "carry")?;
    if output.timed_out {
        bail!("carry timed out after {}s", opts.timeout_secs);
    }
    if !output.status.success() {
        bail!(
            "carry failed before the target could answer: {}{}{}",
            clip(output.stderr.trim(), DIAGNOSTIC_CAP_CHARS),
            if output.stdout.trim().is_empty() {
                ""
            } else {
                "\nstdout: "
            },
            clip(output.stdout.trim(), DIAGNOSTIC_CAP_CHARS)
        );
    }
    serde_json::from_str(&output.stdout).context("carry did not return the expected JSON result")
}

fn run_captured(
    mut cmd: Command,
    input: Option<String>,
    deadline: Instant,
    label: &str,
) -> Result<Captured> {
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("could not start `{label}`"))?;
    let stdout = child
        .stdout
        .take()
        .context("child stdout was unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("child stderr was unavailable")?;
    let stdout_reader = std::thread::spawn(move || read_all(stdout));
    let stderr_reader = std::thread::spawn(move || read_all(stderr));

    if let Some(input) = input {
        let mut stdin = child.stdin.take().context("child stdin was unavailable")?;
        if let Err(error) = stdin.write_all(input.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            bail!("could not send input to {label}: {error}");
        }
    }

    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().context("could not wait for the child")? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break child.wait().context("could not stop the timed-out child")?;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("child stdout reader panicked"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("child stderr reader panicked"))?;
    Ok(Captured {
        status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out,
    })
}

fn success_json(opts: &Options, result: &Answer) -> Value {
    json!({
        "ok": true,
        "to": opts.to.label(),
        "from": result.carry.get("from").cloned().unwrap_or(Value::Null),
        "source_session": match &opts.source {
            Source::Session(session) => Value::String(session.clone()),
            Source::From(_) => Value::Null,
        },
        "session": result.carry.get("id").cloned().unwrap_or(Value::Null),
        "path": result.carry.get("path").cloned().unwrap_or(Value::Null),
        "cwd": result.carry.get("cwd").cloned().unwrap_or(Value::Null),
        "resume": result.carry.get("resume").cloned().unwrap_or(Value::Null),
        "handle": result.carry.get("handle").cloned().unwrap_or(Value::Null),
        "name": result.carry.get("name").cloned().unwrap_or(Value::Null),
        "trail": result.carry.get("trail").cloned().unwrap_or(Value::Null),
        "snapshot": result.carry.get("snapshot").cloned().unwrap_or(Value::Null),
        "receipt": result.carry.get("receipt").cloned().unwrap_or(Value::Null),
        "fork": true,
        "question_chars": result.question_chars,
        "answer": result.answer,
        "elapsed_ms": result.elapsed_ms,
    })
}

fn read_all(mut stream: impl Read) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out);
    out
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n[truncated by constant]");
    out
}

fn diagnostic_suffix(diagnostics: &str) -> String {
    if diagnostics.is_empty() {
        String::new()
    } else {
        format!("\nstderr: {diagnostics}")
    }
}

fn term_safe_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn term_safe_block(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\t' => c,
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

fn shell_words(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            if word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./<>".contains(c))
            {
                word.clone()
            } else {
                format!("{word:?}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn parsing_accepts_flags_and_trailing_question_text() {
        let opts = parse(&args(
            "--to claude --session fixture-01 --json -- Review this approach",
        ))
        .unwrap();
        assert_eq!(opts.to, Runtime::Claude);
        assert!(opts.json);
        assert_eq!(opts.question, "Review this approach");
        assert!(matches!(opts.source, Source::Session(ref s) if s == "fixture-01"));
    }

    #[test]
    fn parsing_rejects_two_source_selectors_and_bad_targets() {
        assert!(
            parse(&args(
                "--to claude --session a --from codex --question hello"
            ))
            .is_err()
        );
        assert!(parse(&args("--to gemini --from codex --question hello")).is_err());
        assert!(parse(&args("--to opencode --from codex --question hello")).is_err());
    }

    #[test]
    fn trailing_question_keeps_spaces_and_punctuation() {
        let opts = parse(&[
            "--to".into(),
            "codex".into(),
            "--from".into(),
            "claude".into(),
            "Is".into(),
            "this".into(),
            "safe?".into(),
        ])
        .unwrap();
        assert_eq!(opts.question, "Is this safe?");
    }
}
