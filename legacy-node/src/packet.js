const fs = require("node:fs");
const path = require("node:path");
const { ensureDir, tryGitState } = require("./fs-util");
const { cleanText, truncate } = require("./redact");

function compilePacket(threadState, options = {}) {
  const targetRuntime = options.targetRuntime || "unknown";
  const recentLimit = Number(options.recentLimit || 12);
  const recentTurns = threadState.events
    .filter((event) => event.role === "user" || event.role === "assistant")
    .slice(-recentLimit);
  const toolEvents = threadState.events
    .filter((event) => event.role === "tool")
    .slice(-12);
  const lastUser = [...threadState.events]
    .reverse()
    .find((event) => event.role === "user");
  const git = tryGitState(threadState.cwd);
  const gitBranch = threadState.gitBranch || git.branch;
  const changedFiles = git.changedFiles || [];
  const summary = latestSummary(threadState);
  const manualNote = cleanText(options.note || "", 2000);

  const lines = [];
  lines.push("# Constant Continuation Packet");
  lines.push("");
  lines.push(
    `You are continuing a conversation that began in ${displayRuntime(
      threadState.sourceRuntime
    )}.`
  );
  lines.push("");
  lines.push("## Current Goal");
  if (lastUser) {
    lines.push(cleanText(truncate(lastUser.text, 1200), 1200));
  } else {
    lines.push("No recent user request was found in the source runtime session.");
  }
  lines.push("");

  lines.push("## What Happened So Far");
  if (summary) {
    lines.push(summary);
  } else {
    lines.push(
      "No durable source summary was found. Use the recent raw turns and workspace state below as the source of truth."
    );
  }
  lines.push("");

  lines.push("## Recent Raw Turns");
  if (recentTurns.length === 0) {
    lines.push("No user or assistant turns were extracted.");
  } else {
    for (const event of recentTurns) {
      lines.push(`### ${capitalize(event.role)}${event.timestamp ? ` - ${event.timestamp}` : ""}`);
      lines.push("");
      lines.push(cleanText(truncate(event.text, 2500), 2500));
      lines.push("");
    }
  }

  lines.push("## Tool Activity");
  if (toolEvents.length === 0) {
    lines.push("No recent tool calls were extracted.");
  } else {
    for (const event of toolEvents) {
      const name = event.toolName || "tool";
      const timestamp = event.timestamp ? ` (${event.timestamp})` : "";
      lines.push(`- ${name}${timestamp}: ${cleanText(truncate(event.text, 350), 350)}`);
    }
  }
  lines.push("");

  lines.push("## Decisions");
  lines.push("No deterministic decision extractor has run yet. Treat explicit user instructions and the recent turns as authoritative.");
  lines.push("");

  lines.push("## Open Questions");
  lines.push("No deterministic open-question extractor has run yet.");
  lines.push("");

  if (manualNote) {
    lines.push("## Manual Bridge Note");
    lines.push(manualNote);
    lines.push("");
  }

  lines.push("## Workspace State");
  lines.push(`cwd: ${threadState.cwd || "unknown"}`);
  lines.push(`git branch: ${gitBranch || (git.isGitRepo ? "unknown" : "not a git repository")}`);
  if (changedFiles.length > 0) {
    lines.push("changed files:");
    for (const changedFile of changedFiles.slice(0, 50)) {
      lines.push(`- ${changedFile}`);
    }
    if (changedFiles.length > 50) {
      lines.push(`- ... ${changedFiles.length - 50} more`);
    }
  } else {
    lines.push(`changed files: ${git.isGitRepo ? "none" : "unavailable"}`);
  }
  lines.push("");

  lines.push("## Source Runtime Metadata");
  lines.push(`source runtime: ${threadState.sourceRuntime}`);
  lines.push(`source session id: ${threadState.sourceSessionId || "unknown"}`);
  lines.push(`source session path: ${threadState.sourcePath || "unknown"}`);
  lines.push(`target runtime: ${targetRuntime}`);
  if (threadState.title) lines.push(`title: ${cleanText(threadState.title, 300)}`);
  if (threadState.model) lines.push(`model: ${threadState.model}`);
  if (threadState.createdAt) lines.push(`created at: ${threadState.createdAt}`);
  if (threadState.updatedAt) lines.push(`updated at: ${threadState.updatedAt}`);
  appendMetadata(lines, threadState.metadata);
  lines.push("");

  lines.push("## Runtime Notes");
  lines.push(
    `Constant does not claim native shared memory. It read visible local ${displayRuntime(
      threadState.sourceRuntime
    )} session evidence, normalized it, and compiled this packet for ${displayRuntime(
      targetRuntime
    )}. Continue from this packet faithfully.`
  );
  lines.push("");

  lines.push("## User's Next Request");
  lines.push("Continue the thread from the state above. Ask for clarification only when the packet lacks necessary state.");
  lines.push("");

  return lines.join("\n");
}

function writePacket(packet, options = {}) {
  const root = options.root || process.cwd();
  const packetDir = options.packetDir || path.join(root, ".constant", "packets");
  ensureDir(packetDir);
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const from = options.from || "unknown";
  const to = options.to || "unknown";
  const filePath =
    options.out ||
    path.join(packetDir, `${timestamp}-${safePart(from)}-to-${safePart(to)}.md`);
  ensureDir(path.dirname(filePath));
  fs.writeFileSync(filePath, packet, "utf8");
  return filePath;
}

function latestSummary(threadState) {
  const summaries = (threadState.summaries || []).filter(isUsefulSummary);
  if (summaries.length === 0) return "";
  return cleanText(summaries[summaries.length - 1], 5000);
}

function isUsefulSummary(summary) {
  if (typeof summary !== "string") return Boolean(summary);
  const normalized = summary.trim().toLowerCase();
  return normalized !== "" && normalized !== "auto" && normalized !== "none";
}

function appendMetadata(lines, metadata = {}) {
  const entries = Object.entries(metadata).filter(
    ([, value]) => value !== undefined && value !== null && value !== ""
  );
  for (const [key, value] of entries) {
    lines.push(`${toKebabLabel(key)}: ${cleanText(String(value), 300)}`);
  }
}

function displayRuntime(runtime) {
  if (runtime === "codex") return "Codex";
  if (runtime === "claude") return "Claude Code";
  if (!runtime) return "the source runtime";
  return String(runtime);
}

function capitalize(value) {
  return value ? `${value[0].toUpperCase()}${value.slice(1)}` : "";
}

function safePart(value) {
  return String(value || "unknown").replace(/[^a-z0-9_-]+/gi, "-").toLowerCase();
}

function toKebabLabel(value) {
  return String(value)
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/_/g, " ")
    .toLowerCase();
}

module.exports = {
  compilePacket,
  writePacket,
};
