const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  commandExists,
  readJsonLines,
  statMtimeMs,
  walkFiles,
} = require("../fs-util");
const { cleanText } = require("../redact");

function codexHome() {
  return process.env.CODEX_HOME || path.join(os.homedir(), ".codex");
}

function discoverCodexSessionFiles(home = codexHome()) {
  const sessionsRoot = path.join(home, "sessions");
  return walkFiles(sessionsRoot, (filePath) => filePath.endsWith(".jsonl")).sort();
}

function readCodexThreadsFromState(home = codexHome(), limit = 25) {
  const statePath = path.join(home, "state_5.sqlite");
  if (!fs.existsSync(statePath) || !commandExists("sqlite3")) return [];

  const sql = `
    select
      id,
      rollout_path as rolloutPath,
      cwd,
      title,
      git_branch as gitBranch,
      git_sha as gitSha,
      git_origin_url as gitOriginUrl,
      model,
      model_provider as modelProvider,
      source,
      cli_version as cliVersion,
      sandbox_policy as sandboxPolicy,
      approval_mode as approvalMode,
      tokens_used as tokensUsed,
      created_at as createdAt,
      updated_at as updatedAt,
      created_at_ms as createdAtMs,
      updated_at_ms as updatedAtMs,
      preview
    from threads
    where archived = 0
    order by coalesce(updated_at_ms, updated_at * 1000, created_at_ms, created_at * 1000) desc
    limit ${Number(limit) || 25};
  `;

  try {
    const json = execFileSync("sqlite3", ["-readonly", "-json", statePath, sql], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    return JSON.parse(json || "[]");
  } catch {
    return [];
  }
}

function listCodexSessions(options = {}) {
  const home = options.home || codexHome();
  const limit = options.limit || 10;
  const fromState = readCodexThreadsFromState(home, limit);
  if (fromState.length > 0) {
    return fromState.map((row) => ({
      runtime: "codex",
      sessionId: row.id,
      path: row.rolloutPath,
      cwd: row.cwd,
      gitBranch: row.gitBranch || undefined,
      title: row.title || row.preview || undefined,
      model: row.model || undefined,
      updatedAt: normalizeTimestamp(row.updatedAtMs || row.updatedAt),
      metadata: row,
    }));
  }

  return discoverCodexSessionFiles(home)
    .map((filePath) => ({
      runtime: "codex",
      sessionId: sessionIdFromCodexPath(filePath),
      path: filePath,
      updatedAt: new Date(statMtimeMs(filePath)).toISOString(),
    }))
    .sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))
    .slice(0, limit);
}

function readCodexLatest(options = {}) {
  const [latest] = listCodexSessions({ ...options, limit: 1 });
  if (!latest) {
    throw new Error(`no Codex sessions found under ${options.home || codexHome()}`);
  }
  return readCodexSession(latest.path, {
    home: options.home,
    stateMetadata: latest.metadata,
  });
}

function readCodexSession(filePath, options = {}) {
  if (!filePath || !fs.existsSync(filePath)) {
    throw new Error(`Codex rollout not found: ${filePath || "(missing path)"}`);
  }

  const rows = readJsonLines(filePath);
  const metadata = options.stateMetadata || {};
  const state = {
    sourceRuntime: "codex",
    sourceSessionId: metadata.id || sessionIdFromCodexPath(filePath),
    sourcePath: filePath,
    title: metadata.title || metadata.preview || undefined,
    cwd: metadata.cwd,
    gitBranch: metadata.gitBranch || undefined,
    model: metadata.model || undefined,
    createdAt: normalizeTimestamp(metadata.createdAtMs || metadata.createdAt),
    updatedAt: normalizeTimestamp(metadata.updatedAtMs || metadata.updatedAt),
    events: [],
    summaries: [],
    metadata: {
      cliVersion: metadata.cliVersion,
      modelProvider: metadata.modelProvider,
      source: metadata.source,
      sandboxPolicy: metadata.sandboxPolicy,
      approvalMode: metadata.approvalMode,
      tokensUsed: metadata.tokensUsed,
    },
  };

  for (const row of rows) {
    const timestamp = row.timestamp;
    if (row.type === "session_meta" && row.payload) {
      state.sourceSessionId = state.sourceSessionId || row.payload.id;
      state.cwd = state.cwd || row.payload.cwd;
      state.createdAt = state.createdAt || timestamp || row.payload.timestamp;
      state.metadata.cliVersion = state.metadata.cliVersion || row.payload.cli_version;
      state.metadata.modelProvider =
        state.metadata.modelProvider || row.payload.model_provider;
      state.metadata.source = state.metadata.source || row.payload.source;
      continue;
    }

    if (row.type === "turn_context" && row.payload) {
      state.cwd = state.cwd || row.payload.cwd;
      state.model = state.model || row.payload.model;
      state.updatedAt = timestamp || state.updatedAt;
      state.metadata.sandboxPolicy =
        state.metadata.sandboxPolicy || row.payload.sandbox_policy;
      state.metadata.approvalMode =
        state.metadata.approvalMode || row.payload.approval_policy;
      if (isUsefulSummary(row.payload.summary)) {
        state.summaries.push(cleanText(row.payload.summary, 5000));
      }
      continue;
    }

    if (row.type !== "response_item" || !row.payload) continue;

    const event = codexResponseItemToEvent(row.payload, {
      runtime: "codex",
      sessionId: state.sourceSessionId,
      timestamp,
      cwd: state.cwd,
      gitBranch: state.gitBranch,
    });

    if (event) {
      state.events.push(event);
      state.updatedAt = timestamp || state.updatedAt;
    }
  }

  return state;
}

function codexResponseItemToEvent(payload, base) {
  if (payload.type === "message") {
    const role = normalizeRole(payload.role);
    if (!role || role === "system") return null;
    const text = cleanText(textFromContent(payload.content), 6000);
    if (!text) return null;
    return {
      ...base,
      role,
      text,
      metadata: {
        nativeType: payload.type,
      },
    };
  }

  if (payload.type === "function_call") {
    return {
      ...base,
      role: "tool",
      toolName: payload.name || "tool",
      text: summarizeToolCall(payload),
      metadata: {
        nativeType: payload.type,
        callId: payload.call_id,
      },
    };
  }

  return null;
}

function textFromContent(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((part) => {
      if (typeof part === "string") return part;
      if (!part || typeof part !== "object") return "";
      return part.text || part.content || part.message || "";
    })
    .filter(Boolean)
    .join("\n");
}

function summarizeToolCall(payload) {
  let args = payload.arguments;
  if (typeof args === "string") {
    try {
      args = JSON.parse(args);
    } catch {
      args = { arguments: args };
    }
  }

  if (!args || typeof args !== "object") {
    return cleanText(`${payload.name || "tool"} called`, 500);
  }

  const details = [];
  for (const key of ["cmd", "command", "path", "file_path", "workdir", "query"]) {
    if (args[key]) details.push(`${key}: ${args[key]}`);
  }
  const suffix = details.length > 0 ? ` (${details.join("; ")})` : "";
  return cleanText(`${payload.name || "tool"} called${suffix}`, 700);
}

function normalizeRole(role) {
  if (role === "user" || role === "assistant") return role;
  if (role === "tool") return "tool";
  return null;
}

function isUsefulSummary(summary) {
  if (typeof summary !== "string") return Boolean(summary);
  const normalized = summary.trim().toLowerCase();
  return normalized !== "" && normalized !== "auto" && normalized !== "none";
}

function sessionIdFromCodexPath(filePath) {
  const base = path.basename(filePath, ".jsonl");
  const match = base.match(
    /^rollout-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-(.+)$/
  );
  return match ? match[1] : base;
}

function normalizeTimestamp(value) {
  if (!value) return undefined;
  if (typeof value === "string" && Number.isNaN(Number(value))) return value;
  const number = Number(value);
  if (!Number.isFinite(number) || number <= 0) return undefined;
  const milliseconds = number > 10_000_000_000 ? number : number * 1000;
  return new Date(milliseconds).toISOString();
}

module.exports = {
  codexHome,
  discoverCodexSessionFiles,
  listCodexSessions,
  readCodexLatest,
  readCodexSession,
};
