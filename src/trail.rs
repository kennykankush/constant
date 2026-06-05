//! Trail — human-readable lineage for Constant's session projections.
//!
//! Opaque session ids make it impossible to track which file is which after a
//! few switches. The trail gives each Constant-owned projection a readable name
//! (`constant·tNN·from-<src>·<root-slug>`), which we stamp into the runtime's
//! native resume picker, and logs every switch to `~/.constant/trail.jsonl` so
//! `constant trail` can reconstruct the lineage.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::runtime::Runtime;

/// Slugify a conversation's first message into a short, readable handle:
/// lowercase alphanumerics, first ~6 words, joined by `-`.
pub fn slug(name: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for raw in name.split_whitespace() {
        let w: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if !w.is_empty() {
            words.push(w);
        }
        if words.len() >= 6 {
            break;
        }
    }
    if words.is_empty() {
        "conversation".to_string()
    } else {
        words.join("-")
    }
}

/// The native title we stamp on a projection, e.g.
/// `constant·t01·from-codex·can-you-help-me`.
pub fn title(n: u32, from: Runtime, root_slug: &str) -> String {
    format!("constant·t{n:02}·from-{}·{root_slug}", from.label())
}

fn ledger_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".constant");
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("trail.jsonl"))
}

/// Append one switch to the trail ledger. Best-effort: never fails a switch.
///
/// `conv_id` is the stable grouping key (the conversation's root source session
/// id) — durable and unique, so unrelated threads never merge. `slug` is only
/// the human display handle (which can collide and is not used for grouping).
#[allow(clippy::too_many_arguments)]
pub fn record(
    n: u32,
    conv_id: &str,
    slug: &str,
    cwd: Option<&Path>,
    from: Runtime,
    to: Runtime,
    id: &str,
    path: &Path,
    title: &str,
) {
    let Some(ledger) = ledger_path() else {
        return;
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = serde_json::json!({
        "ts": ts,
        "n": n,
        "conversation": conv_id,
        "slug": slug,
        "cwd": cwd.map(|p| p.display().to_string()),
        "from": from.label(),
        "to": to.label(),
        "id": id,
        "path": path.display().to_string(),
        "title": title,
    });
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&ledger) {
        let _ = writeln!(f, "{entry}");
    }
}

/// On (re)host, recover the conversation a source belongs to and the trail
/// number to continue from, by consulting the ledger. If the source we're about
/// to carry was itself a projection we wrote before, reuse that conversation key
/// so the lineage stays together (and native `tNN` titles keep counting up)
/// across re-hosts; otherwise the source id starts a fresh conversation. Returns
/// (conversation_id, last_trail_number, prior_projections). The projections are
/// the latest Constant-owned session per target runtime for this conversation
/// (existing files only), so the caller can seed its `owned` map and reuse the
/// stable pair across re-hosts instead of minting new files.
#[allow(clippy::type_complexity)]
pub fn resume(src_path: &Path, src_id: &str) -> (String, u32, Vec<(Runtime, String, PathBuf)>) {
    let fallback = (src_id.to_string(), 0u32, Vec::new());
    let Some(ledger) = ledger_path() else {
        return fallback;
    };
    let Ok(text) = fs::read_to_string(&ledger) else {
        return fallback;
    };
    let src_path_str = src_path.display().to_string();

    // Which conversation does this source belong to? If it matches a prior
    // projection's id or path, reuse that conversation key; else it's new.
    let mut conv_id = src_id.to_string();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let matches = v.get("id").and_then(|x| x.as_str()) == Some(src_id)
            || v.get("path").and_then(|x| x.as_str()) == Some(src_path_str.as_str());
        if matches
            && let Some(c) = v.get("conversation").and_then(|x| x.as_str()) {
                conv_id = c.to_string();
                break;
            }
    }

    // Continue the trail number after the highest recorded for this conversation,
    // and recover the latest projection per target runtime (one pass).
    let mut max_n = 0u32;
    let mut latest: std::collections::HashMap<String, (u64, String, String)> =
        std::collections::HashMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("conversation").and_then(|x| x.as_str()) != Some(conv_id.as_str()) {
            continue;
        }
        let n = v.get("n").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        max_n = max_n.max(n);
        let (Some(to), Some(id), Some(path)) = (
            v.get("to").and_then(|x| x.as_str()),
            v.get("id").and_then(|x| x.as_str()),
            v.get("path").and_then(|x| x.as_str()),
        ) else {
            continue;
        };
        let ts = v.get("ts").and_then(|x| x.as_u64()).unwrap_or(0);
        if latest.get(to).map(|(t, _, _)| ts >= *t).unwrap_or(true) {
            latest.insert(to.to_string(), (ts, id.to_string(), path.to_string()));
        }
    }

    let mut projections = Vec::new();
    for (to, (_, id, path)) in latest {
        if let Ok(rt) = Runtime::parse(&to) {
            let p = PathBuf::from(&path);
            if p.exists() {
                projections.push((rt, id, p));
            }
        }
    }

    (conv_id, max_n, projections)
}

/// `constant trail` — print the lineage grouped by conversation. Filters to
/// `cwd_filter` when given (default: the current directory).
pub fn print(cwd_filter: Option<&Path>) -> anyhow::Result<()> {
    use std::collections::HashMap;

    let Some(ledger) = ledger_path() else {
        println!("no trail yet");
        return Ok(());
    };
    let Ok(text) = fs::read_to_string(&ledger) else {
        println!("no trail yet ({})", ledger.display());
        return Ok(());
    };
    let want_cwd = cwd_filter.map(|p| p.display().to_string());

    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(ref want) = want_cwd
            && v.get("cwd").and_then(|c| c.as_str()) != Some(want.as_str()) {
                continue;
            }
        let conv = v
            .get("conversation")
            .and_then(|c| c.as_str())
            .unwrap_or("?")
            .to_string();
        if !groups.contains_key(&conv) {
            order.push(conv.clone());
        }
        groups.entry(conv).or_default().push(v);
    }

    // Stable order within a group: by timestamp (the ledger is append-only, so
    // this is also insertion order). Display re-numbers sequentially so that a
    // re-hosted conversation never shows duplicate t-numbers.
    for entries in groups.values_mut() {
        entries.sort_by_key(|e| e.get("ts").and_then(|t| t.as_u64()).unwrap_or(0));
    }

    if order.is_empty() {
        match want_cwd {
            Some(c) => println!(
                "no trail yet for {}\n(try `constant trail --all`)",
                crate::term_safe(&c)
            ),
            None => println!("no trail yet"),
        }
        return Ok(());
    }

    for conv in order {
        let entries = &groups[&conv];
        // All of these are ledger/path-derived — sanitize control chars before
        // printing, since cwd/paths can contain ESC/OSC bytes (terminal injection).
        let cwd = crate::term_safe(
            entries
                .first()
                .and_then(|e| e.get("cwd"))
                .and_then(|c| c.as_str())
                .unwrap_or(""),
        );
        // Prefer the readable slug for the header; fall back to the id.
        let display = crate::term_safe(
            entries
                .iter()
                .find_map(|e| e.get("slug").and_then(|s| s.as_str()))
                .unwrap_or(conv.as_str()),
        );
        println!("\nconversation: {display}   ({cwd})");
        for (i, e) in entries.iter().enumerate() {
            let n = i + 1; // re-numbered on display, stable across host runs
            let from = crate::term_safe(e.get("from").and_then(|v| v.as_str()).unwrap_or("?"));
            let to = crate::term_safe(e.get("to").and_then(|v| v.as_str()).unwrap_or("?"));
            let title = crate::term_safe(e.get("title").and_then(|v| v.as_str()).unwrap_or(""));
            println!("  t{n:02}  {from:>6} \u{2192} {to:<6}  {title}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_takes_first_six_words_lowercased() {
        assert_eq!(slug("ok can you help me do me a favour"), "ok-can-you-help-me-do");
        assert_eq!(slug("Fix THE Bug!!"), "fix-the-bug");
    }

    #[test]
    fn slug_falls_back_when_empty() {
        assert_eq!(slug(""), "conversation");
        assert_eq!(slug("!!! ??? ***"), "conversation");
    }

    #[test]
    fn title_format_is_stable() {
        assert_eq!(
            title(1, Runtime::Codex, "can-you-help"),
            "constant·t01·from-codex·can-you-help"
        );
        assert_eq!(
            title(12, Runtime::Claude, "x"),
            "constant·t12·from-claude·x"
        );
    }
}
