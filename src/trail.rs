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

#[derive(Clone, Debug)]
struct TrailEntry {
    ts: u64,
    n: u32,
    conversation: String,
    slug: String,
    cwd: Option<String>,
    from: String,
    to: String,
    source_id: Option<String>,
    source_path: Option<String>,
    id: String,
    path: String,
    title: String,
    mode: Option<String>,
    snapshot: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProjectionView {
    pub runtime: String,
    pub id: String,
    pub title: String,
    pub last_from: String,
    pub last_n: u32,
    pub refreshes: usize,
    pub older_projection_count: usize,
}

#[derive(Clone, Debug)]
pub struct ConversationView {
    pub conversation: String,
    pub slug: String,
    pub cwd: Option<String>,
    pub entries: usize,
    pub last_ts: u64,
    pub projections: Vec<ProjectionView>,
}

#[derive(Clone, Debug)]
pub struct RouteNodeView {
    pub alias: String,
    pub runtime: String,
    pub id: String,
    pub path: String,
    pub title: String,
    pub parent_alias: String,
    pub mode: String,
    pub last_from: String,
    pub last_n: u32,
    pub refreshes: usize,
    pub active: bool,
}

#[derive(Clone, Debug)]
pub struct RouteConversationView {
    pub conversation: String,
    pub slug: String,
    pub cwd: Option<String>,
    pub root_alias: String,
    pub root_runtime: String,
    pub entries: usize,
    pub last_ts: u64,
    pub nodes: Vec<RouteNodeView>,
}

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
    Some(PathBuf::from(home).join(".constant").join("trail.jsonl"))
}

fn ledger_path_for_write() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".constant");
    let _ = fs::create_dir_all(&dir);
    restrict_dir(&dir);
    Some(dir.join("trail.jsonl"))
}

/// The record vault aggregates every conversation in one place — keep it
/// owner-only on unix.
fn restrict_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
}

/// A conversation id used as a directory name: anything outside
/// [A-Za-z0-9._-] becomes `-` so a hostile id can't traverse paths.
fn safe_dir_component(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.').to_string();
    if trimmed.is_empty() {
        "conversation".to_string()
    } else {
        trimmed
    }
}

/// Where this hop's record volume lives:
/// `~/.constant/snapshots/<conversation>/tNN-from-<runtime>.json`.
/// Creates the directories (owner-only).
pub fn snapshot_path(conv_id: &str, n: u32, from: Runtime) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let constant = PathBuf::from(home).join(".constant");
    let _ = fs::create_dir_all(&constant);
    restrict_dir(&constant);
    let snapshots = constant.join("snapshots");
    let dir = snapshots.join(safe_dir_component(conv_id));
    let _ = fs::create_dir_all(&dir);
    restrict_dir(&snapshots);
    restrict_dir(&dir);
    Some(dir.join(format!("t{n:02}-from-{}.json", from.label())))
}

fn parse_entry(line: &str) -> Option<TrailEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(TrailEntry {
        ts: v.get("ts").and_then(|x| x.as_u64()).unwrap_or(0),
        n: v.get("n").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        conversation: v
            .get("conversation")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string(),
        slug: v
            .get("slug")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string(),
        cwd: v.get("cwd").and_then(|x| x.as_str()).map(str::to_string),
        from: v
            .get("from")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string(),
        to: v
            .get("to")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string(),
        source_id: v
            .get("source_id")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        source_path: v
            .get("source_path")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        id: v
            .get("target_id")
            .or_else(|| v.get("id"))
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string(),
        path: v
            .get("target_path")
            .or_else(|| v.get("path"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        title: v
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        mode: v.get("mode").and_then(|x| x.as_str()).map(str::to_string),
        snapshot: v
            .get("snapshot")
            .and_then(|x| x.as_str())
            .map(str::to_string),
    })
}

/// Compare a recorded cwd string against a filter path, tolerant of symlinked
/// spellings of the same directory (the rest of the codebase canonicalizes —
/// the ledger views must not split one project into two over a symlink).
fn same_cwd(recorded: &str, want: &Path) -> bool {
    if recorded == want.display().to_string() {
        return true;
    }
    match (Path::new(recorded).canonicalize(), want.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn load_entries(cwd_filter: Option<&Path>) -> Vec<TrailEntry> {
    let Some(ledger) = ledger_path() else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(&ledger) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(parse_entry)
        .filter(|e| match cwd_filter {
            Some(want) => e.cwd.as_deref().map(|c| same_cwd(c, want)).unwrap_or(false),
            None => true,
        })
        .collect()
}

fn runtime_rank(runtime: &str) -> u8 {
    match runtime {
        "codex" => 0,
        "claude" => 1,
        "opencode" => 2,
        "gemini" => 3,
        _ => 9,
    }
}

fn resume_cmd(runtime: &str, id: &str) -> String {
    match runtime {
        "codex" => format!("codex resume {id}"),
        "claude" => format!("claude -r {id}"),
        "gemini" => format!("gemini --resume {id}"),
        "opencode" => format!("opencode -s {id}"),
        _ => format!("{runtime} resume {id}"),
    }
}

pub fn conversations(cwd_filter: Option<&Path>) -> Vec<ConversationView> {
    use std::collections::{HashMap, HashSet};

    let mut grouped: HashMap<String, Vec<TrailEntry>> = HashMap::new();
    for entry in load_entries(cwd_filter) {
        grouped
            .entry(entry.conversation.clone())
            .or_default()
            .push(entry);
    }

    let mut out = Vec::new();
    for (conversation, mut entries) in grouped {
        entries.sort_by_key(|e| (e.ts, e.n));
        let slug = entries
            .iter()
            .find(|e| e.slug != "?")
            .map(|e| e.slug.clone())
            .unwrap_or_else(|| conversation.clone());
        let cwd = entries.iter().find_map(|e| e.cwd.clone());
        let last_ts = entries.iter().map(|e| e.ts).max().unwrap_or(0);

        let mut latest_by_runtime: HashMap<String, TrailEntry> = HashMap::new();
        let mut distinct_by_runtime: HashMap<String, HashSet<String>> = HashMap::new();
        for e in &entries {
            // The current projection view should only advertise resumable files.
            // The append-only evidence remains available in `trail --events`.
            if e.path.is_empty() || !PathBuf::from(&e.path).exists() {
                continue;
            }
            distinct_by_runtime
                .entry(e.to.clone())
                .or_default()
                .insert(format!("{}\n{}", e.id, e.path));
            if latest_by_runtime
                .get(&e.to)
                .map(|old| (e.ts, e.n) >= (old.ts, old.n))
                .unwrap_or(true)
            {
                latest_by_runtime.insert(e.to.clone(), e.clone());
            }
        }

        let mut projections: Vec<ProjectionView> = latest_by_runtime
            .into_iter()
            .map(|(runtime, latest)| {
                let refreshes = entries
                    .iter()
                    .filter(|e| e.to == runtime && e.id == latest.id && e.path == latest.path)
                    .count();
                let distinct = distinct_by_runtime
                    .get(&runtime)
                    .map(|s| s.len())
                    .unwrap_or(1);
                ProjectionView {
                    runtime,
                    id: latest.id,
                    title: latest.title,
                    last_from: latest.from,
                    last_n: latest.n,
                    refreshes,
                    older_projection_count: distinct.saturating_sub(1),
                }
            })
            .collect();
        projections.sort_by_key(|p| runtime_rank(&p.runtime));

        out.push(ConversationView {
            conversation,
            slug,
            cwd,
            entries: entries.len(),
            last_ts,
            projections,
        });
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.last_ts));
    out
}

fn node_key(runtime: &str, id: &str, path: &str) -> String {
    format!("{runtime}\n{id}\n{path}")
}

fn node_id_key(runtime: &str, id: &str) -> String {
    format!("{runtime}\n{id}")
}

fn alias_path(alias: &str) -> String {
    alias
        .split_once('[')
        .and_then(|(_, rest)| rest.strip_suffix(']'))
        .unwrap_or("1")
        .to_string()
}

pub fn route_views(cwd_filter: Option<&Path>) -> Vec<RouteConversationView> {
    use std::collections::HashMap;

    let mut grouped: HashMap<String, Vec<TrailEntry>> = HashMap::new();
    for entry in load_entries(cwd_filter) {
        grouped
            .entry(entry.conversation.clone())
            .or_default()
            .push(entry);
    }

    let mut out = Vec::new();
    for (conversation, mut entries) in grouped {
        entries.sort_by_key(|e| (e.ts, e.n));
        let Some(first) = entries.first().cloned() else {
            continue;
        };
        let slug = entries
            .iter()
            .find(|e| e.slug != "?")
            .map(|e| e.slug.clone())
            .unwrap_or_else(|| conversation.clone());
        let cwd = entries.iter().find_map(|e| e.cwd.clone());
        let last_ts = entries.iter().map(|e| e.ts).max().unwrap_or(0);
        let root_runtime = first.from.clone();
        let root_alias = format!("{root_runtime}[1]");
        let root_key = node_key(&root_runtime, &conversation, "");

        let mut aliases: HashMap<String, String> = HashMap::new();
        let mut aliases_by_id: HashMap<String, String> = HashMap::new();
        aliases.insert(root_key.clone(), root_alias.clone());
        aliases_by_id.insert(
            node_id_key(&root_runtime, &conversation),
            root_alias.clone(),
        );
        let mut child_counts: HashMap<String, usize> = HashMap::new();
        let mut nodes: Vec<RouteNodeView> = Vec::new();
        let mut node_index: HashMap<String, usize> = HashMap::new();

        for e in &entries {
            let source_exact_key = e
                .source_id
                .as_deref()
                .map(|source_id| {
                    node_key(&e.from, source_id, e.source_path.as_deref().unwrap_or(""))
                })
                .unwrap_or_else(|| root_key.clone());
            let source_id_key = e
                .source_id
                .as_deref()
                .map(|source_id| node_id_key(&e.from, source_id));

            let parent_alias = if let Some(alias) = aliases.get(&source_exact_key) {
                alias.clone()
            } else if let Some(id_key) = source_id_key
                .as_ref()
                .filter(|id_key| aliases_by_id.contains_key(*id_key))
            {
                aliases_by_id
                    .get(id_key)
                    .cloned()
                    .unwrap_or_else(|| root_alias.clone())
            } else {
                // Older ledger rows did not record the exact source node. Treat
                // them as carries from the root so historical trails still render.
                root_alias.clone()
            };

            let target_key = node_key(&e.to, &e.id, &e.path);
            if let Some(idx) = node_index.get(&target_key).copied() {
                let node = &mut nodes[idx];
                node.refreshes += 1;
                node.last_from = e.from.clone();
                node.last_n = e.n;
                node.title = e.title.clone();
                node.active = !e.path.is_empty() && PathBuf::from(&e.path).exists();
                node.mode = e
                    .mode
                    .clone()
                    .unwrap_or_else(|| "refresh-existing".to_string());
                continue;
            }

            let child_n = child_counts.entry(parent_alias.clone()).or_insert(0);
            *child_n += 1;
            let path = format!("{}.{}", alias_path(&parent_alias), child_n);
            let alias = format!("{}[{path}]", e.to);
            aliases.insert(target_key.clone(), alias.clone());
            aliases_by_id.insert(node_id_key(&e.to, &e.id), alias.clone());
            node_index.insert(target_key, nodes.len());
            nodes.push(RouteNodeView {
                alias,
                runtime: e.to.clone(),
                id: e.id.clone(),
                path: e.path.clone(),
                title: e.title.clone(),
                parent_alias,
                mode: e.mode.clone().unwrap_or_else(|| "new-fork".to_string()),
                last_from: e.from.clone(),
                last_n: e.n,
                refreshes: 1,
                active: !e.path.is_empty() && PathBuf::from(&e.path).exists(),
            });
        }

        out.push(RouteConversationView {
            conversation,
            slug,
            cwd,
            root_alias,
            root_runtime,
            entries: entries.len(),
            last_ts,
            nodes,
        });
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.last_ts));
    out
}

/// Append one switch to the trail ledger.
///
/// `conv_id` is the stable grouping key (the conversation's root source session
/// id) — durable and unique, so unrelated threads never merge. `slug` is only
/// the human display handle (which can collide and is not used for grouping).
///
/// The ledger is what pair-reuse and lineage recovery are rebuilt from: a
/// failed append means the next re-host can silently fork the conversation, so
/// the error is RETURNED for the caller to surface (the switch itself still
/// proceeds — divergence is recoverable, a dead harness is not).
#[allow(clippy::too_many_arguments)]
pub fn record(
    n: u32,
    conv_id: &str,
    slug: &str,
    cwd: Option<&Path>,
    source_id: &str,
    source_path: &Path,
    from: Runtime,
    to: Runtime,
    id: &str,
    path: &Path,
    title: &str,
    mode: &str,
    snapshot: Option<&Path>,
) -> anyhow::Result<()> {
    let ledger =
        ledger_path_for_write().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Store the canonical spelling so symlinked invocations of the same
    // project group as one conversation in the scoped views.
    let cwd = cwd.map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
    let entry = serde_json::json!({
        "ts": ts,
        "n": n,
        "conversation": conv_id,
        "slug": slug,
        "cwd": cwd.map(|p| p.display().to_string()),
        "from": from.label(),
        "to": to.label(),
        "source_id": source_id,
        "source_path": source_path.display().to_string(),
        "id": id,
        "path": path.display().to_string(),
        "target_id": id,
        "target_path": path.display().to_string(),
        "title": title,
        "mode": mode,
        "snapshot": snapshot.map(|p| p.display().to_string()),
    });
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger)?;
    writeln!(f, "{entry}")?;
    Ok(())
}

/// On (re)host, recover the conversation a source belongs to and the trail
/// number to continue from, by consulting the ledger. If the source we're about
/// to carry was itself a projection we wrote before, reuse that conversation key
/// so the lineage stays together (and native `tNN` titles keep counting up)
/// across re-hosts; otherwise the source id starts a fresh conversation. Returns
/// (conversation_id, last_trail_number, prior_projections). The projections are
/// scoped to the current source node: direct children from this source plus its
/// owned parent projection when switching back. This keeps sibling `--new`
/// branches separate instead of collapsing them by target runtime for the whole
/// conversation.
#[allow(clippy::type_complexity)]
pub fn resume(src_path: &Path, src_id: &str) -> (String, u32, Vec<(Runtime, String, PathBuf)>) {
    let fallback = (src_id.to_string(), 0u32, Vec::new());
    let Some(ledger) = ledger_path() else {
        return fallback;
    };
    let Ok(text) = fs::read_to_string(&ledger) else {
        return fallback;
    };
    let entries: Vec<TrailEntry> = text.lines().filter_map(parse_entry).collect();
    let (conv_id, max_n, projections) = resume_from_entries(&entries, src_path, src_id);
    // Only advertise projections that still exist on disk (IO stays out of the
    // pure core so it can be unit-tested against synthetic ledgers).
    let projections = projections
        .into_iter()
        .filter(|(_, _, p)| p.exists())
        .collect();
    (conv_id, max_n, projections)
}

/// The pure ledger-reconciliation core of [`resume`] — the most intricate
/// logic in the codebase, kept IO-free so the unit tests can drive it with
/// synthetic ledgers.
#[allow(clippy::type_complexity)]
fn resume_from_entries(
    entries: &[TrailEntry],
    src_path: &Path,
    src_id: &str,
) -> (String, u32, Vec<(Runtime, String, PathBuf)>) {
    let src_path_str = src_path.display().to_string();

    // Which conversation does this source belong to? If it matches a prior
    // projection's id or path, reuse that conversation key; else it's new.
    let mut conv_id = src_id.to_string();
    for e in entries {
        if e.id == src_id || e.path == src_path_str {
            conv_id = e.conversation.clone();
            break;
        }
    }

    // Continue the trail number after the highest recorded for this conversation,
    // and recover reusable projections tied to this source node.
    let mut max_n = 0u32;
    let mut latest: std::collections::HashMap<String, (u64, String, String)> =
        std::collections::HashMap::new();

    let conv_entries: Vec<&TrailEntry> = entries
        .iter()
        .filter(|e| e.conversation == conv_id)
        .collect();

    for e in &conv_entries {
        max_n = max_n.max(e.n);
        let direct_child = e.source_id.as_deref() == Some(src_id)
            || e.source_path.as_deref() == Some(src_path_str.as_str());
        if !direct_child {
            continue;
        }
        if latest
            .get(&e.to)
            .map(|(t, _, _)| e.ts >= *t)
            .unwrap_or(true)
        {
            latest.insert(e.to.clone(), (e.ts, e.id.clone(), e.path.clone()));
        }
    }

    // If this source is itself a Constant-owned projection, remember its owned
    // parent projection so a normal ping-pong switch returns to that same parent
    // instead of minting a parallel branch.
    for child in &conv_entries {
        if child.id != src_id && child.path != src_path_str {
            continue;
        }
        let (Some(parent_id), Some(parent_path)) = (&child.source_id, &child.source_path) else {
            continue;
        };
        for parent in &conv_entries {
            let parent_matches =
                parent.to == child.from && (parent.id == *parent_id || parent.path == *parent_path);
            if parent_matches
                && latest
                    .get(&parent.to)
                    .map(|(t, _, _)| parent.ts >= *t)
                    .unwrap_or(true)
            {
                latest.insert(
                    parent.to.clone(),
                    (parent.ts, parent.id.clone(), parent.path.clone()),
                );
            }
        }
    }

    // Backward compatibility: older trail rows did not record source_id/source_path,
    // so they cannot be scoped to a source node. Preserve the old stable-pair
    // behavior for runtimes that do not have scoped evidence yet.
    let scoped_runtimes: std::collections::HashSet<String> = latest.keys().cloned().collect();
    for e in &conv_entries {
        if e.source_id.is_some() || e.source_path.is_some() || scoped_runtimes.contains(&e.to) {
            continue;
        }
        if latest
            .get(&e.to)
            .map(|(t, _, _)| e.ts >= *t)
            .unwrap_or(true)
        {
            latest.insert(e.to.clone(), (e.ts, e.id.clone(), e.path.clone()));
        }
    }

    let mut projections = Vec::new();
    for (to, (_, id, path)) in latest {
        if let Ok(rt) = Runtime::parse(&to) {
            projections.push((rt, id, PathBuf::from(&path)));
        }
    }

    (conv_id, max_n, projections)
}

/// `constant trail --events` — print the append-only switch ledger grouped by
/// conversation. Filters to `cwd_filter` when given (default: current dir).
pub fn print_events(cwd_filter: Option<&Path>) -> anyhow::Result<()> {
    use std::collections::HashMap;

    let entries = load_entries(cwd_filter);
    let want_cwd = cwd_filter.map(|p| p.display().to_string());
    if entries.is_empty() {
        match want_cwd {
            Some(c) => println!(
                "no trail events for {}\n(try `constant trail --all --events`)",
                crate::term_safe(&c)
            ),
            None => println!("no trail events yet"),
        }
        return Ok(());
    }

    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<TrailEntry>> = HashMap::new();
    for entry in entries {
        if !groups.contains_key(&entry.conversation) {
            order.push(entry.conversation.clone());
        }
        groups
            .entry(entry.conversation.clone())
            .or_default()
            .push(entry);
    }

    for entries in groups.values_mut() {
        entries.sort_by_key(|e| (e.ts, e.n));
    }

    for conv in order {
        let entries = &groups[&conv];
        let cwd = crate::term_safe(entries.first().and_then(|e| e.cwd.as_deref()).unwrap_or(""));
        let display = crate::term_safe(
            entries
                .iter()
                .find(|e| e.slug != "?")
                .map(|e| e.slug.as_str())
                .unwrap_or(conv.as_str()),
        );
        println!("\nconversation: {display}   ({cwd})");
        for (i, e) in entries.iter().enumerate() {
            let n = i + 1; // display order, not necessarily ledger n
            let from = crate::term_safe(&e.from);
            let to = crate::term_safe(&e.to);
            let id = crate::term_safe(&e.id);
            let title = crate::term_safe(&e.title);
            println!("  t{n:02}  {from:>6} \u{2192} {to:<6}  {id}  {title}");
        }
    }
    Ok(())
}

/// The newest record volume for a conversation that still exists on disk —
/// the lost-record fallback for `constant resume`: when every live projection
/// is gone, the conversation is reprinted from its latest record.
pub fn latest_snapshot(conv_id: &str) -> Option<PathBuf> {
    let mut rows: Vec<TrailEntry> = load_entries(None)
        .into_iter()
        .filter(|e| e.conversation == conv_id && e.snapshot.is_some())
        .collect();
    rows.sort_by_key(|e| std::cmp::Reverse((e.ts, e.n)));
    rows.into_iter()
        .filter_map(|e| e.snapshot.map(PathBuf::from))
        .find(|p| p.exists())
}

/// `constant snapshots` — list the record volumes (per-hop IR snapshots) the
/// ledger knows about, grouped by conversation. A volume the ledger doesn't
/// reference is effectively lost in the archive, so this lists from the
/// ledger and marks files that have since gone missing.
pub fn print_snapshots(cwd_filter: Option<&Path>) -> anyhow::Result<()> {
    use std::collections::HashMap;

    let entries: Vec<TrailEntry> = load_entries(cwd_filter)
        .into_iter()
        .filter(|e| e.snapshot.is_some())
        .collect();
    if entries.is_empty() {
        match cwd_filter {
            Some(c) => println!(
                "no records for {}\n(records are written at every carry; try `constant snapshots --all`)",
                crate::term_safe(&c.display().to_string())
            ),
            None => println!("no records yet (records are written at every carry)"),
        }
        return Ok(());
    }

    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<TrailEntry>> = HashMap::new();
    for entry in entries {
        if !groups.contains_key(&entry.conversation) {
            order.push(entry.conversation.clone());
        }
        groups
            .entry(entry.conversation.clone())
            .or_default()
            .push(entry);
    }

    for conv in order {
        let rows = groups.get_mut(&conv).unwrap();
        rows.sort_by_key(|e| (e.ts, e.n));
        let display = crate::term_safe(
            rows.iter()
                .find(|e| e.slug != "?")
                .map(|e| e.slug.as_str())
                .unwrap_or(conv.as_str()),
        );
        let cwd = crate::term_safe(rows.first().and_then(|e| e.cwd.as_deref()).unwrap_or(""));
        println!("\nconversation: {display}   ({cwd})");
        for e in rows.iter() {
            let snap = e.snapshot.as_deref().unwrap_or("");
            let status = if Path::new(snap).exists() {
                "ok     "
            } else {
                "missing"
            };
            println!(
                "  t{:02}  from {:<6}  {status}  {}",
                e.n,
                crate::term_safe(&e.from),
                crate::term_safe(snap)
            );
        }
        if let Some(last) = rows.iter().rev().find(|e| {
            e.snapshot
                .as_deref()
                .map(|s| Path::new(s).exists())
                .unwrap_or(false)
        }) {
            println!(
                "  restore latest: constant restore {}",
                crate::term_safe(last.snapshot.as_deref().unwrap_or(""))
            );
        }
    }
    Ok(())
}

/// `constant route` — print the fork graph Constant can reconstruct from the
/// trail ledger. This is the debugging view for "which projection did I make?".
pub fn print_routes(
    cwd_filter: Option<&Path>,
    conversation_filter: Option<&str>,
) -> anyhow::Result<()> {
    let mut views = route_views(cwd_filter);
    if let Some(want) = conversation_filter {
        views.retain(|v| v.conversation == want);
    }

    let want_cwd = cwd_filter.map(|p| p.display().to_string());
    if views.is_empty() {
        match (want_cwd, conversation_filter) {
            (_, Some(want)) => println!("no route for {}", crate::term_safe(want)),
            (Some(c), None) => println!(
                "no routes for {}\n(try `constant route --all`)",
                crate::term_safe(&c)
            ),
            (None, None) => println!("no routes yet"),
        }
        return Ok(());
    }

    for conv in views {
        let cwd = crate::term_safe(conv.cwd.as_deref().unwrap_or(""));
        let display = crate::term_safe(&conv.slug);
        let root = crate::term_safe(&conv.conversation);
        println!("\nconversation: {display}   ({cwd})");
        println!(
            "  root: {}  {} {}",
            crate::term_safe(&conv.root_alias),
            crate::term_safe(&conv.root_runtime),
            root
        );
        if conv.nodes.is_empty() {
            println!("  routes: none");
            continue;
        }
        for node in &conv.nodes {
            let active = if node.active { "active" } else { "missing" };
            let refresh = if node.refreshes > 1 {
                format!("synced {}x", node.refreshes)
            } else {
                "synced 1x".to_string()
            };
            println!(
                "  {} -> {}  {}  {}",
                crate::term_safe(&node.parent_alias),
                crate::term_safe(&node.alias),
                crate::term_safe(&node.id),
                active
            );
            println!(
                "       last: t{:02} from {} ({refresh}, {})",
                node.last_n,
                crate::term_safe(&node.last_from),
                crate::term_safe(&node.mode)
            );
            if !node.title.is_empty() {
                println!("       title: {}", crate::term_safe(&node.title));
            }
            if !node.path.is_empty() {
                println!("       path: {}", crate::term_safe(&node.path));
            }
            println!(
                "       resume: {}",
                crate::term_safe(&resume_cmd(&node.runtime, &node.id))
            );
        }
        if conv.entries > conv.nodes.len() {
            println!("  events: {} (`constant trail --events`)", conv.entries);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn print_carry_debug(
    src_path: &Path,
    src_id: &str,
    conv_id: &str,
    slug: &str,
    from: Runtime,
    to: Runtime,
    n: u32,
    reuse: Option<(&str, &Path)>,
) {
    println!("route debug");
    println!("  source: {} {}", from.label(), crate::term_safe(src_id));
    println!(
        "          {}",
        crate::term_safe(&src_path.display().to_string())
    );
    println!(
        "  conversation: {}  {}",
        crate::term_safe(slug),
        crate::term_safe(conv_id)
    );
    println!("  target runtime: {}", to.label());
    match reuse {
        Some((id, path)) => {
            println!("  action: refresh-existing");
            println!(
                "  intended projection: {} {}",
                to.label(),
                crate::term_safe(id)
            );
            println!(
                "                       {}",
                crate::term_safe(&path.display().to_string())
            );
        }
        None => {
            println!("  action: new-fork");
            println!("  intended projection: pending new {} fork", to.label());
        }
    }
    println!("  next event: t{n:02}");
    println!();
}

/// `constant trail` — print current conversation projections, not the raw event
/// ledger. This is the user-facing view: one target runtime projection per
/// conversation, with a refresh count when the same projection was updated more
/// than once.
pub fn print(cwd_filter: Option<&Path>) -> anyhow::Result<()> {
    let views = conversations(cwd_filter);
    let want_cwd = cwd_filter.map(|p| p.display().to_string());
    if views.is_empty() {
        match want_cwd {
            Some(c) => println!(
                "no trail yet for {}\n(try `constant trail --all`)",
                crate::term_safe(&c)
            ),
            None => println!("no trail yet"),
        }
        return Ok(());
    }

    for conv in views {
        let cwd = crate::term_safe(conv.cwd.as_deref().unwrap_or(""));
        let display = crate::term_safe(&conv.slug);
        let root = crate::term_safe(&conv.conversation);
        println!("\nconversation: {display}   ({cwd})");
        println!("  root: {root}");
        if conv.projections.is_empty() {
            println!("  projections: none");
        } else {
            for p in &conv.projections {
                let runtime = crate::term_safe(&p.runtime);
                let id = crate::term_safe(&p.id);
                let title = crate::term_safe(&p.title);
                let resume = crate::term_safe(&resume_cmd(&p.runtime, &p.id));
                let refresh = if p.refreshes > 1 {
                    format!("synced {}x", p.refreshes)
                } else {
                    "synced 1x".to_string()
                };
                let older = if p.older_projection_count > 0 {
                    format!(", {} older", p.older_projection_count)
                } else {
                    String::new()
                };
                println!("  {runtime:<6} {id}  {title}");
                println!(
                    "         last: t{:02} from {} ({refresh}{older})",
                    p.last_n,
                    crate::term_safe(&p.last_from)
                );
                println!("         resume: {resume}");
            }
        }
        if conv.entries > conv.projections.len() {
            println!("  events: {} (`constant trail --events`)", conv.entries);
        }
    }
    Ok(())
}

pub fn print_status(cwd_filter: Option<&Path>) -> anyhow::Result<()> {
    let views = conversations(cwd_filter);
    if views.is_empty() {
        println!("trail: none");
        return Ok(());
    }

    println!("trail:");
    for conv in views.iter().take(3) {
        let root = crate::term_safe(&conv.conversation);
        println!("  conversation {root}");
        for p in &conv.projections {
            println!(
                "    {:<6} {}  {}",
                crate::term_safe(&p.runtime),
                crate::term_safe(&p.id),
                if p.refreshes > 1 {
                    format!("synced {}x", p.refreshes)
                } else {
                    "synced 1x".to_string()
                }
            );
        }
    }
    if views.len() > 3 {
        println!("  ... {} more (`constant trail --all`)", views.len() - 3);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_takes_first_six_words_lowercased() {
        assert_eq!(
            slug("ok can you help me do me a favour"),
            "ok-can-you-help-me-do"
        );
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

    // --- resume_from_entries: the ledger-reconciliation core ---

    fn entry(json: serde_json::Value) -> TrailEntry {
        parse_entry(&json.to_string()).expect("test entry parses")
    }

    /// One switch row: conversation `conv`, hop `n` at time `ts`,
    /// from (`from`, `sid`@`spath`) into (`to`, `tid`@`tpath`).
    #[allow(clippy::too_many_arguments)]
    fn row(
        conv: &str,
        n: u32,
        ts: u64,
        from: &str,
        sid: &str,
        spath: &str,
        to: &str,
        tid: &str,
        tpath: &str,
    ) -> TrailEntry {
        entry(serde_json::json!({
            "ts": ts, "n": n, "conversation": conv, "slug": "s", "cwd": "/p",
            "from": from, "to": to,
            "source_id": sid, "source_path": spath,
            "target_id": tid, "target_path": tpath,
            "title": "t", "mode": "new-fork",
        }))
    }

    #[test]
    fn resume_unknown_source_starts_fresh_conversation() {
        let (conv, n, projs) = resume_from_entries(&[], Path::new("/x/s.jsonl"), "S");
        assert_eq!(conv, "S");
        assert_eq!(n, 0);
        assert!(projs.is_empty());
    }

    #[test]
    fn resume_ping_pong_keeps_a_stable_pair() {
        // The live switch sequence: codex S → claude C (t1), claude C → codex A2
        // (t2: a NEW codex projection — never the user's original S).
        let ledger = vec![
            row("S", 1, 100, "codex", "S", "/s", "claude", "C", "/c"),
            row("S", 2, 200, "claude", "C", "/c", "codex", "A2", "/a2"),
        ];

        // Switching OUT of codex A2 must come back to claude C (the owned
        // parent), not mint a parallel claude branch.
        let (conv, n, projs) = resume_from_entries(&ledger, Path::new("/a2"), "A2");
        assert_eq!(conv, "S");
        assert_eq!(n, 2);
        assert_eq!(projs.len(), 1, "{projs:?}");
        assert!(matches!(&projs[0], (Runtime::Claude, id, _) if id == "C"));

        // And switching OUT of claude C must reuse codex A2 (its direct child),
        // never the original seed S.
        let (conv, _, projs) = resume_from_entries(&ledger, Path::new("/c"), "C");
        assert_eq!(conv, "S");
        let codex: Vec<_> = projs
            .iter()
            .filter(|(rt, _, _)| *rt == Runtime::Codex)
            .collect();
        assert_eq!(codex.len(), 1, "{projs:?}");
        assert!(matches!(codex[0], (_, id, _) if id == "A2"));
    }

    #[test]
    fn resume_sibling_branches_stay_separate() {
        // Two --new claude siblings from the same codex source: a continue from
        // the SOURCE refreshes the newest sibling; a continue from sibling C1
        // must not see C2's codex child.
        let ledger = vec![
            row("S", 1, 100, "codex", "S", "/s", "claude", "C1", "/c1"),
            row("S", 2, 200, "codex", "S", "/s", "claude", "C2", "/c2"),
            row("S", 3, 300, "claude", "C2", "/c2", "codex", "X2", "/x2"),
        ];

        let (_, n, projs) = resume_from_entries(&ledger, Path::new("/s"), "S");
        assert_eq!(n, 3);
        let claude: Vec<_> = projs
            .iter()
            .filter(|(rt, _, _)| *rt == Runtime::Claude)
            .collect();
        assert!(matches!(claude[0], (_, id, _) if id == "C2"), "{projs:?}");

        let (_, _, projs) = resume_from_entries(&ledger, Path::new("/c1"), "C1");
        assert!(
            !projs.iter().any(|(_, id, _)| id == "X2"),
            "sibling C1 must not adopt C2's codex child: {projs:?}"
        );
    }

    #[test]
    fn resume_matches_projection_by_id_when_path_moved() {
        let ledger = vec![row(
            "S", 1, 100, "codex", "S", "/s", "claude", "C", "/c-original",
        )];
        // Same projection id surfacing from a different path still joins its
        // conversation instead of starting a new one.
        let (conv, n, _) = resume_from_entries(&ledger, Path::new("/c-moved"), "C");
        assert_eq!(conv, "S");
        assert_eq!(n, 1);
    }
}
