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
    handle: Option<String>,
    name: Option<String>,
    named: Option<bool>,
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
    /// The stable address (`cobalt-37`).
    pub handle: String,
    /// The glance title (rename > harvested > birth slug).
    pub name: String,
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
    // Humans open conversations with throat-clearing; names shouldn't.
    const FILLER: [&str; 22] = [
        "ok", "okay", "hey", "hi", "hello", "so", "wait", "can", "could", "you",
        "u", "please", "pls", "um", "uh", "like", "just", "now", "right", "lets",
        "also", "and",
    ];
    let mut words: Vec<String> = Vec::new();
    let mut content_started = false;
    for raw in name.split_whitespace() {
        let w: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if w.is_empty() {
            continue;
        }
        if !content_started && FILLER.contains(&w.as_str()) {
            continue; // leading filler only — once content starts, keep everything
        }
        content_started = true;
        words.push(w);
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

/// The native title we stamp on a projection — leads with the human name,
/// then the chapter, provenance, and the stable handle:
/// `auth redirect bug · ch04 ← codex · cobalt-37`.
pub fn title(n: u32, from: Runtime, name: &str, handle: &str) -> String {
    format!("{name} \u{b7} ch{n:02} \u{2190} {} \u{b7} {handle}", from.label())
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
/// [A-Za-z0-9_-] becomes `-` — no separators, no dots, so a hostile id can
/// never traverse paths or even *resemble* a traversal.
fn safe_dir_component(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.chars().all(|c| c == '-') {
        "conversation".to_string()
    } else {
        cleaned
    }
}

/// The handle lexicon: ~100 clean single-token color words. Shape is ONE
/// color + a 2-digit tail (`cobalt-37`) — deliberately unlike opencode's
/// adjective-noun pairs, so the two conventions can never be confused.
const COLORS: [&str; 96] = [
    "amber", "ash", "azure", "basalt", "beige", "bronze", "brass", "burgundy",
    "carmine", "celadon", "cerise", "cerulean", "charcoal", "cinnabar", "citrine", "claret",
    "cobalt", "copper", "coral", "cream", "crimson", "cyan", "denim", "ebony",
    "ecru", "emerald", "fawn", "flax", "fuchsia", "garnet", "ginger", "gold",
    "graphite", "gunmetal", "hazel", "heather", "henna", "indigo", "iris", "ivory",
    "jade", "jasper", "jet", "lavender", "lilac", "lime", "linen", "madder",
    "magenta", "mahogany", "maroon", "mauve", "mint", "moss", "mulberry", "mustard",
    "ochre", "olive", "onyx", "opal", "orchid", "pearl", "periwinkle", "pewter",
    "pine", "plum", "porcelain", "puce", "pumpkin", "quartz", "raspberry", "rose",
    "ruby", "russet", "rust", "saffron", "sage", "salmon", "sand", "sapphire",
    "scarlet", "sepia", "sienna", "silver", "slate", "smoke", "steel", "tan",
    "taupe", "teal", "terracotta", "topaz", "turquoise", "umber", "vermilion", "viridian",
];

/// Mint the handle for a conversation: the hash SUGGESTS (`sha256(conv_id)`
/// picks a color and a 2-digit tail, deterministically), the LEDGER decides —
/// if another conversation already pinned that handle, the tail extends with
/// further hash digits until free. Once written into a ledger row the handle
/// is a registry fact: collisions are impossible by construction, not by
/// probability.
fn mint_handle(conv_id: &str, entries: &[TrailEntry]) -> String {
    let hash = crate::alembic::sha256::hex(conv_id);
    let bytes = hash.as_bytes();
    let color = COLORS[(bytes[0] as usize * 256 + bytes[1] as usize) % COLORS.len()];
    // Digits derived from successive hash chars, so lengthening is deterministic.
    let hex_digits: String = hash
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                c
            } else {
                (((c as u8 - b'a') % 10) + b'0') as char
            }
        })
        .collect();
    let taken_by_other = |candidate: &str| {
        entries
            .iter()
            .any(|e| e.handle.as_deref() == Some(candidate) && e.conversation != conv_id)
    };
    let mut len = 2;
    loop {
        let tail: String = hex_digits.chars().skip(2).take(len).collect();
        let candidate = format!("{color}-{tail}");
        if !taken_by_other(&candidate) {
            return candidate;
        }
        len += 1;
    }
}

/// A conversation's naming state, resolved from the ledger.
#[derive(Clone, Debug)]
pub struct Naming {
    /// The stable address (`cobalt-37`) — pinned at first carry, never changes.
    pub handle: String,
    /// The semantic title — the glance layer.
    pub name: String,
    /// True once a human explicitly renamed it (auto-naming then stops forever).
    pub named: bool,
}

/// Resolve (or mint) the naming for a conversation. `birth` is the smart
/// birth-slug fallback; `harvested` is a runtime-generated title when one
/// exists (opencode's titles, a claude /rename) — used only while the
/// conversation hasn't been explicitly named (auto until touched).
pub fn naming_for(conv_id: &str, birth: &str, harvested: Option<&str>) -> Naming {
    let entries: Vec<TrailEntry> = ledger_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|t| t.lines().filter_map(parse_entry).collect())
        .unwrap_or_default();
    naming_from_entries(&entries, conv_id, birth, harvested)
}

/// The pure core of [`naming_for`], unit-testable against synthetic ledgers.
fn naming_from_entries(
    entries: &[TrailEntry],
    conv_id: &str,
    birth: &str,
    harvested: Option<&str>,
) -> Naming {
    let handle = entries
        .iter()
        .find(|e| e.conversation == conv_id && e.handle.is_some())
        .and_then(|e| e.handle.clone())
        .unwrap_or_else(|| mint_handle(conv_id, entries));

    // Latest recorded name wins; an explicit rename locks it forever.
    let mut name: Option<String> = None;
    let mut named = false;
    for e in entries.iter().filter(|e| e.conversation == conv_id) {
        if let Some(n) = &e.name {
            name = Some(n.clone());
            if e.named.unwrap_or(false) {
                named = true;
            }
        }
    }
    let name = if named {
        name.unwrap_or_else(|| birth.to_string())
    } else {
        harvested
            .map(str::to_string)
            .or(name)
            .unwrap_or_else(|| birth.to_string())
    };

    Naming {
        handle,
        name,
        named,
    }
}

/// Where this hop's record volume lives:
/// `~/.constant/snapshots/<conversation>/chNN-from-<runtime>.json`.
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
    Some(dir.join(format!("ch{n:02}-from-{}.json", from.label())))
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
        handle: v
            .get("handle")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        name: v.get("name").and_then(|x| x.as_str()).map(str::to_string),
        named: v.get("named").and_then(|x| x.as_bool()),
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
        let naming = naming_from_entries(&entries, &conversation, &slug, None);
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
            handle: naming.handle,
            name: naming.name,
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
pub struct CarryRow<'a> {
    pub n: u32,
    pub conv_id: &'a str,
    pub slug: &'a str,
    pub cwd: Option<&'a Path>,
    pub source_id: &'a str,
    pub source_path: &'a Path,
    pub from: Runtime,
    pub to: Runtime,
    pub id: &'a str,
    pub path: &'a Path,
    pub title: &'a str,
    pub mode: &'a str,
    pub snapshot: Option<&'a Path>,
    /// The conversation's pinned naming at this hop.
    pub handle: &'a str,
    pub name: &'a str,
    pub named: bool,
}

/// Record an explicit rename — an append-only naming event. An explicit
/// rename locks the title forever (auto-naming stops: "auto until touched").
pub fn record_rename(
    conv_id: &str,
    handle: &str,
    new_name: &str,
    cwd: Option<&Path>,
) -> anyhow::Result<()> {
    let ledger = ledger_path_for_write().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cwd = cwd.map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
    let entry = serde_json::json!({
        "ts": ts,
        "conversation": conv_id,
        "handle": handle,
        "name": new_name,
        "named": true,
        "mode": "rename",
        "slug": slug(new_name),
        "cwd": cwd.map(|p| p.display().to_string()),
    });
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger)?;
    writeln!(f, "{entry}")?;
    Ok(())
}

pub fn record(row: &CarryRow) -> anyhow::Result<()> {
    let CarryRow {
        n,
        conv_id,
        slug,
        cwd,
        source_id,
        source_path,
        from,
        to,
        id,
        path,
        title,
        mode,
        snapshot,
        handle,
        name,
        named,
    } = *row;
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
        "handle": handle,
        "name": name,
        "named": named,
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
            println!("  ch{n:02}  {from:>6} \u{2192} {to:<6}  {id}  {title}");
        }
    }
    Ok(())
}

/// One hop of a conversation, shaped for the control-room graph.
#[derive(Clone, Debug)]
pub struct ChapterRow {
    pub n: u32,
    pub from: String,
    pub to: String,
    pub ts: u64,
    pub mode: String,
    /// The hop's record volume still exists on disk.
    pub recorded: bool,
}

/// The chapters of a conversation, oldest first (rename events excluded).
pub fn chapters(conv_id: &str) -> Vec<ChapterRow> {
    let mut rows: Vec<ChapterRow> = load_entries(None)
        .into_iter()
        .filter(|e| e.conversation == conv_id && e.mode.as_deref() != Some("rename"))
        .map(|e| ChapterRow {
            n: e.n,
            from: e.from.clone(),
            to: e.to.clone(),
            ts: e.ts,
            mode: e.mode.clone().unwrap_or_else(|| "carry".to_string()),
            recorded: e
                .snapshot
                .as_deref()
                .map(|p| Path::new(p).exists())
                .unwrap_or(false),
        })
        .collect();
    rows.sort_by_key(|r| (r.ts, r.n));
    rows
}

/// Raw ledger lines (verbatim) for one conversation — the pack's row payload.
/// Verbatim strings, not re-serialized values: the pack carries exactly what
/// the ledger said.
pub fn raw_rows(conv_id: &str) -> Vec<String> {
    ledger_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|t| {
            t.lines()
                .filter(|l| {
                    parse_entry(l)
                        .map(|e| e.conversation == conv_id)
                        .unwrap_or(false)
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The local vault directory for a conversation's record volumes (created,
/// owner-only) — where `unpack` lands imported volumes.
pub fn vault_dir(conv_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let constant = PathBuf::from(home).join(".constant");
    let _ = fs::create_dir_all(&constant);
    restrict_dir(&constant);
    let snapshots = constant.join("snapshots");
    let dir = snapshots.join(safe_dir_component(conv_id));
    let _ = fs::create_dir_all(&dir);
    restrict_dir(&snapshots);
    restrict_dir(&dir);
    Some(dir)
}

pub struct UnpackSummary {
    pub handle: String,
    /// True when the packed handle was already pinned to a DIFFERENT
    /// conversation locally — the registry re-minted one (never two owners).
    pub rehandled: bool,
    pub rows_added: usize,
    pub rows_skipped: usize,
}

/// Import packed ledger rows for a conversation onto THIS machine:
/// - snapshot paths are rewritten to the local vault (`volume_paths`)
/// - the handle is re-minted if the local registry already gave it away
/// - rows already present locally are skipped (idempotent unpack)
pub fn import_rows(
    conv_id: &str,
    packed: &[String],
    volume_paths: &std::collections::HashMap<String, PathBuf>,
) -> anyhow::Result<UnpackSummary> {
    let local: Vec<TrailEntry> = ledger_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|t| t.lines().filter_map(parse_entry).collect())
        .unwrap_or_default();
    let existing: std::collections::HashSet<(String, u64, u32, String)> = local
        .iter()
        .map(|e| (e.conversation.clone(), e.ts, e.n, e.id.clone()))
        .collect();

    let pack_handle = packed
        .iter()
        .filter_map(|l| parse_entry(l))
        .find_map(|e| e.handle);
    let conflict = pack_handle
        .as_ref()
        .map(|h| {
            local
                .iter()
                .any(|e| e.handle.as_deref() == Some(h) && e.conversation != conv_id)
        })
        .unwrap_or(false);
    let handle = if conflict || pack_handle.is_none() {
        mint_handle(conv_id, &local)
    } else {
        pack_handle.clone().unwrap()
    };

    let ledger = ledger_path_for_write().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger)?;
    let mut rows_added = 0usize;
    let mut rows_skipped = 0usize;
    for line in packed {
        let Some(e) = parse_entry(line) else {
            rows_skipped += 1;
            continue;
        };
        if e.conversation != conv_id
            || existing.contains(&(e.conversation.clone(), e.ts, e.n, e.id.clone()))
        {
            rows_skipped += 1;
            continue;
        }
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else {
            rows_skipped += 1;
            continue;
        };
        // Uniform handle on imported rows (registry law: one owner).
        v["handle"] = serde_json::Value::String(handle.clone());
        // A pack carries the RECORD, not projections: the source machine's
        // projection paths mean nothing here (and could even collide with
        // local files). Blank them — the conversation wakes via restore.
        for key in ["path", "target_path"] {
            if v.get(key).is_some() {
                v[key] = serde_json::Value::String(String::new());
            }
        }
        // Snapshot paths follow the volumes to the LOCAL vault.
        if let Some(old) = v.get("snapshot").and_then(serde_json::Value::as_str)
            && let Some(fname) = Path::new(old).file_name().and_then(|n| n.to_str())
            && let Some(local_path) = volume_paths.get(fname)
        {
            v["snapshot"] = serde_json::Value::String(local_path.display().to_string());
        }
        writeln!(f, "{v}")?;
        rows_added += 1;
    }

    Ok(UnpackSummary {
        handle,
        rehandled: conflict,
        rows_added,
        rows_skipped,
    })
}

/// The conversation slug a session id belongs to, if the ledger knows it
/// (as a projection or as a recorded source) — used by `constant ps` to name
/// live processes.
pub fn label_for_session(id: &str) -> Option<String> {
    let entries = load_entries(None);
    let conv = entries.iter().find_map(|e| {
        (e.id == id || e.source_id.as_deref() == Some(id)).then(|| e.conversation.clone())
    })?;
    let slug = entries
        .iter()
        .find(|e| e.conversation == conv && e.slug != "?")
        .map(|e| e.slug.clone())
        .unwrap_or_else(|| conv.clone());
    let naming = naming_from_entries(&entries, &conv, &slug, None);
    Some(format!("{} \u{b7} {}", naming.handle, naming.name))
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
pub fn print_snapshots(cwd_filter: Option<&Path>, full: bool) -> anyhow::Result<()> {
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

    let a = ansi();
    let (dim, bold, reset) = (a.dim, a.bold, a.reset);
    let all_entries = load_entries(None);

    for conv in order {
        let rows = groups.get_mut(&conv).unwrap();
        rows.sort_by_key(|e| (e.ts, e.n));
        let slug = rows
            .iter()
            .find(|e| e.slug != "?")
            .map(|e| e.slug.clone())
            .unwrap_or_else(|| conv.clone());
        let naming = naming_from_entries(&all_entries, &conv, &slug, None);
        let cwd = crate::term_safe(rows.first().and_then(|e| e.cwd.as_deref()).unwrap_or(""));

        println!();
        println!(
            "  {bold}{}{reset} {}  {dim}({cwd}){reset}",
            crate::term_safe(&naming.handle),
            clip(&crate::term_safe(&naming.name), 56)
        );
        for e in rows.iter() {
            let snap = e.snapshot.as_deref().unwrap_or("");
            let exists = Path::new(snap).exists();
            let status = if exists { "ok" } else { "missing" };
            let color = if a.tty { runtime_paint(&e.from) } else { "" };
            print!(
                "    ch{:02} \u{2190} {color}{}{reset}   {status:<7} {dim}{}{reset}",
                e.n,
                crate::term_safe(&e.from),
                ago(e.ts)
            );
            if full {
                print!("  {dim}{}{reset}", crate::term_safe(snap));
            }
            println!();
        }
        if let Some(last) = rows.iter().rev().find(|e| {
            e.snapshot
                .as_deref()
                .map(|s| Path::new(s).exists())
                .unwrap_or(false)
        }) {
            println!(
                "    {dim}\u{21b3} constant restore {}{reset}",
                crate::term_safe(last.snapshot.as_deref().unwrap_or(""))
            );
        }
    }
    if !full {
        println!("\n{dim}volume paths: constant snapshots --full{reset}");
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

    let a = ansi();
    let (dim, reset) = (a.dim, a.reset);
    for conv in views {
        let cwd = crate::term_safe(conv.cwd.as_deref().unwrap_or(""));
        let display = crate::term_safe(&conv.slug);
        let root = crate::term_safe(&conv.conversation);
        println!("\nconversation: {display}   {dim}({cwd}){reset}");
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
                "       last: ch{:02} from {} ({refresh}, {})",
                node.last_n,
                crate::term_safe(&node.last_from),
                crate::term_safe(&node.mode)
            );
            if !node.title.is_empty() {
                println!("       {dim}title: {}{reset}", clip(&crate::term_safe(&node.title), 64));
            }
            if !node.path.is_empty() {
                println!("       {dim}path: {}{reset}", crate::term_safe(&node.path));
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
/// ANSI styling for views, gated on stdout being a terminal (piped output
/// stays plain for scripts and tests).
pub(crate) struct Ansi {
    pub dim: &'static str,
    pub bold: &'static str,
    pub reset: &'static str,
    pub tty: bool,
}

pub(crate) fn ansi() -> Ansi {
    use std::io::IsTerminal;
    let tty = std::io::stdout().is_terminal();
    Ansi {
        dim: if tty { "\x1b[2m" } else { "" },
        bold: if tty { "\x1b[1m" } else { "" },
        reset: if tty { "\x1b[0m" } else { "" },
        tty,
    }
}

/// Relative age for the compact trail view ("3d ago", "2h ago", "just now").
pub(crate) fn ago(ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dt = now.saturating_sub(ts);
    match dt {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", dt / 60),
        3600..=86_399 => format!("{}h ago", dt / 3600),
        86_400..=2_591_999 => format!("{}d ago", dt / 86_400),
        _ => format!("{}mo ago", dt / 2_592_000),
    }
}

/// Clip a name to `max` display chars with an ellipsis.
pub(crate) fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}\u{2026}")
}

pub(crate) fn runtime_paint(runtime: &str) -> &'static str {
    match runtime {
        "claude" => "\x1b[38;5;208m",
        "codex" => "\x1b[38;5;39m",
        "opencode" => "\x1b[38;5;77m",
        "gemini" => "\x1b[38;5;177m",
        _ => "",
    }
}

/// `constant trail` — the compact, eye-first view. One card per conversation:
/// handle + name + where it stands, the chapter chain colored by narrator,
/// and the one command that matters. Ids, native resume commands, and stamped
/// titles live under `--full`; the raw ledger under `--events`.
pub fn print(cwd_filter: Option<&Path>) -> anyhow::Result<()> {
    let mut views = conversations(cwd_filter);
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
    views.sort_by_key(|v| std::cmp::Reverse(v.last_ts));

    let a = ansi();
    let (tty, dim, bold, reset) = (a.tty, a.dim, a.bold, a.reset);

    match &want_cwd {
        Some(c) => println!(
            "{dim}{} conversation{} \u{b7} {}{reset}",
            views.len(),
            if views.len() == 1 { "" } else { "s" },
            crate::term_safe(c)
        ),
        None => println!(
            "{dim}{} conversation{} \u{b7} everywhere{reset}",
            views.len(),
            if views.len() == 1 { "" } else { "s" }
        ),
    }

    let handle_w = views
        .iter()
        .map(|v| v.handle.chars().count())
        .max()
        .unwrap_or(8)
        .max(8);

    for conv in &views {
        let handle = crate::term_safe(&conv.handle);
        let name = clip(&crate::term_safe(&conv.name), 56);
        let latest_n = conv.projections.iter().map(|p| p.last_n).max().unwrap_or(0);
        let when = ago(conv.last_ts);

        println!();
        print!("  {bold}{handle:<handle_w$}{reset} {name}");
        if latest_n > 0 {
            print!("  {dim}\u{b7} ch{latest_n:02} \u{b7} {when}{reset}");
        } else {
            print!("  {dim}\u{b7} {when}{reset}");
        }
        if want_cwd.is_none()
            && let Some(c) = &conv.cwd
        {
            print!("  {dim}({}){reset}", crate::term_safe(c));
        }
        println!();

        let pad = " ".repeat(handle_w + 3);
        if conv.projections.is_empty() {
            println!("{pad}{dim}no live projections \u{2014} resume reprints from the record{reset}");
        } else {
            let chain = conv
                .projections
                .iter()
                .map(|p| {
                    let color = if tty { runtime_paint(&p.runtime) } else { "" };
                    let times = if p.refreshes > 1 {
                        format!(" \u{d7}{}", p.refreshes)
                    } else {
                        String::new()
                    };
                    let older = if p.older_projection_count > 0 {
                        format!(" (+{} older)", p.older_projection_count)
                    } else {
                        String::new()
                    };
                    format!(
                        "{color}{}{reset} {dim}ch{:02}{times}{older}{reset}",
                        crate::term_safe(&p.runtime),
                        p.last_n
                    )
                })
                .collect::<Vec<_>>()
                .join(&format!(" {dim}\u{2192}{reset} "));
            println!("{pad}{chain}");
        }
        println!("{pad}{dim}\u{21b3} constant resume {handle}{reset}");
    }
    println!("\n{dim}ids \u{b7} native commands \u{b7} stamps: constant trail --full{reset}");
    Ok(())
}

/// `constant trail --full` — the verbose view: ids, stamped titles, native
/// resume commands, root id, event counts.
pub fn print_full(cwd_filter: Option<&Path>) -> anyhow::Result<()> {
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
        let handle = crate::term_safe(&conv.handle);
        let name = crate::term_safe(&conv.name);
        let root = crate::term_safe(&conv.conversation);
        println!("\n{handle} \u{b7} {name}   ({cwd})");
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
                    "         last: ch{:02} from {} ({refresh}{older})",
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
    let mut views = conversations(cwd_filter);
    if views.is_empty() {
        println!("trail: none");
        return Ok(());
    }
    views.sort_by_key(|v| std::cmp::Reverse(v.last_ts));
    let a = ansi();
    let (dim, bold, reset) = (a.dim, a.bold, a.reset);

    println!("trail:");
    let handle_w = views
        .iter()
        .take(3)
        .map(|v| v.handle.chars().count())
        .max()
        .unwrap_or(8)
        .max(8);
    for conv in views.iter().take(3) {
        let chain = conv
            .projections
            .iter()
            .map(|p| {
                let color = if a.tty { runtime_paint(&p.runtime) } else { "" };
                let times = if p.refreshes > 1 {
                    format!(" \u{d7}{}", p.refreshes)
                } else {
                    String::new()
                };
                format!(
                    "{color}{}{reset} {dim}ch{:02}{times}{reset}",
                    crate::term_safe(&p.runtime),
                    p.last_n
                )
            })
            .collect::<Vec<_>>()
            .join(&format!(" {dim}\u{2192}{reset} "));
        println!(
            "  {bold}{:<handle_w$}{reset} {:<42} {}",
            crate::term_safe(&conv.handle),
            clip(&crate::term_safe(&conv.name), 40),
            if chain.is_empty() {
                format!("{dim}no live projections{reset}")
            } else {
                chain
            }
        );
    }
    if views.len() > 3 {
        println!(
            "  {dim}\u{2026} {} more (`constant trail`){reset}",
            views.len() - 3
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_strips_leading_filler_then_takes_six_words() {
        // Throat-clearing is dropped until content starts; after that,
        // everything counts (fillers mid-sentence are kept).
        assert_eq!(
            slug("ok can you help me do me a favour"),
            "help-me-do-me-a-favour"
        );
        assert_eq!(slug("Fix THE Bug!!"), "fix-the-bug");
        assert_eq!(slug("hey so wait lets fix auth"), "fix-auth");
        // All filler: falls back rather than emptying.
        assert_eq!(slug("ok hey so"), "conversation");
    }

    #[test]
    fn slug_falls_back_when_empty() {
        assert_eq!(slug(""), "conversation");
        assert_eq!(slug("!!! ??? ***"), "conversation");
    }

    #[test]
    fn title_format_is_stable() {
        assert_eq!(
            title(1, Runtime::Codex, "auth redirect bug", "cobalt-37"),
            "auth redirect bug \u{b7} ch01 \u{2190} codex \u{b7} cobalt-37"
        );
    }

    // --- handles: hash suggests, the ledger registry decides ---

    #[test]
    fn handle_minting_is_deterministic_and_collision_proof() {
        // Same conversation always proposes the same handle.
        let a = mint_handle("conv-aaaa", &[]);
        let b = mint_handle("conv-aaaa", &[]);
        assert_eq!(a, b);
        assert!(a.contains('-'), "{a}");
        let (color, tail) = a.split_once('-').unwrap();
        assert!(COLORS.contains(&color), "{a}");
        assert_eq!(tail.len(), 2, "{a}");

        // If ANOTHER conversation pinned that exact handle, the tail extends
        // deterministically instead of colliding.
        let taken = entry(serde_json::json!({
            "ts": 1, "conversation": "other-conv", "handle": a,
        }));
        let extended = mint_handle("conv-aaaa", &[taken]);
        assert_ne!(extended, a);
        assert!(extended.starts_with(&format!("{color}-")), "{extended}");
        assert!(extended.len() > a.len(), "{extended}");

        // The conversation that OWNS the handle keeps it.
        let own = entry(serde_json::json!({
            "ts": 1, "conversation": "conv-aaaa", "handle": a,
        }));
        assert_eq!(mint_handle("conv-aaaa", &[own]), a);
    }

    #[test]
    fn naming_precedence_rename_locks_over_harvest() {
        // Fresh conversation: birth slug, unless a harvested title exists.
        let nm = naming_from_entries(&[], "c1", "birth-slug", None);
        assert_eq!(nm.name, "birth-slug");
        assert!(!nm.named);
        let nm = naming_from_entries(&[], "c1", "birth-slug", Some("Fix auth bug"));
        assert_eq!(nm.name, "Fix auth bug");

        // An explicit rename row locks the name: harvest can't override it.
        let renamed = entry(serde_json::json!({
            "ts": 2, "conversation": "c1", "handle": "cobalt-37",
            "name": "my chosen name", "named": true, "mode": "rename",
        }));
        let nm = naming_from_entries(
            &[renamed],
            "c1",
            "birth-slug",
            Some("Harvested title"),
        );
        assert_eq!(nm.name, "my chosen name");
        assert!(nm.named);
        assert_eq!(nm.handle, "cobalt-37");
    }

    #[test]
    fn hostile_conversation_ids_cannot_traverse_paths() {
        // conv ids become directory names in the record vault — a crafted
        // session id must never escape ~/.constant/snapshots.
        assert_eq!(safe_dir_component("../../etc/passwd"), "------etc-passwd");
        assert_eq!(safe_dir_component("a/b\\c"), "a-b-c");
        assert_eq!(safe_dir_component("..."), "conversation");
        assert_eq!(safe_dir_component(""), "conversation");
        assert_eq!(safe_dir_component("normal-id_1.2"), "normal-id_1-2");
        assert!(!safe_dir_component("../../x").contains('.'));
        assert!(!safe_dir_component("a/b").contains('/'));
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
