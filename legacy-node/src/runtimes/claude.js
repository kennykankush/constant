const os = require("node:os");
const path = require("node:path");
const { readJsonLines, statMtimeMs, walkFiles } = require("../fs-util");
const { cleanText } = require("../redact");

function claudeHome() {
  return process.env.CLAUDE_HOME || path.join(os.homedir(), ".claude");
}

function discoverClaudeSessionFiles(home = claudeHome()) {
  const projectsRoot = path.join(home, "projects");
  return walkFiles(
    projectsRoot,
    (filePath) => filePath.endsWith(".jsonl") && !filePath.endsWith("skill-injections.jsonl")
  ).sort();
}

function listClaudeSessions(options = {}) {
  const home = options.home || claudeHome();
  const limit = options.limit || 10;
  return discoverClaudeSessionFiles(home)
    .map((filePath) => ({
      runtime: "claude",
      sessionId: path.basename(filePath, ".jsonl"),
      path: filePath,
      updatedAt: new Date(statMtimeMs(filePath)).toISOString(),
    }))
    .sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))
    .slice(0, limit);
}

function readClaudeLatest(options = {}) {
  const [latest] = listClaudeSessions({ ...options, limit: 1 });
  if (!latest) {
    throw new Error(`no Claude sessions found under ${options.home || claudeHome()}`);
  }
  return readClaudeSession(latest.path);
}

function readClaudeSession(filePath) {
  const rows = readJsonLines(filePath);
  const state = {
    sourceRuntime: "claude",
    sourceSessionId: path.basename(filePath, ".jsonl"),
    sourcePath: filePath,
    events: [],
    summaries: [],
    metadata: {},
  };

  for (const row of rows) {
    if (!row || !row.type) continue;
    if (row.cwd) state.cwd = state.cwd || row.cwd;
    if (row.gitBranch) state.gitBranch = state.gitBranch || row.gitBranch;
    if (row.timestamp) state.updatedAt = row.timestamp;
    if (row.sessionId) state.sourceSessionId = row.sessionId;

    if (row.type !== "user" && row.type !== "assistant") continue;
    const text = cleanText(textFromClaudeMessage(row.message), 6000);
    if (!text) continue;
    state.events.push({
      runtime: "claude",
      sessionId: state.sourceSessionId,
      timestamp: row.timestamp,
      role: row.type,
      text,
      cwd: row.cwd,
      gitBranch: row.gitBranch,
      metadata: {
        nativeType: row.type,
        model: row.message && row.message.model,
      },
    });
  }

  return state;
}

function textFromClaudeMessage(message) {
  if (!message) return "";
  const content = message.content;
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((part) => {
      if (typeof part === "string") return part;
      if (!part || typeof part !== "object") return "";
      if (part.type === "text") return part.text || "";
      if (part.type === "tool_use") return `[tool use: ${part.name || "tool"}]`;
      return "";
    })
    .filter(Boolean)
    .join("\n");
}

module.exports = {
  claudeHome,
  discoverClaudeSessionFiles,
  listClaudeSessions,
  readClaudeLatest,
  readClaudeSession,
};
