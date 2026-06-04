const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { commandExists } = require("./fs-util");
const { compilePacket, writePacket } = require("./packet");
const { cleanText, truncate } = require("./redact");
const {
  codexHome,
  listCodexSessions,
  readCodexLatest,
  readCodexSession,
} = require("./runtimes/codex");
const {
  claudeHome,
  listClaudeSessions,
  readClaudeLatest,
  readClaudeSession,
} = require("./runtimes/claude");

async function main(argv) {
  const { command, flags } = parseArgs(argv);
  if (!command || command === "--help" || command === "-h" || flags.help || flags.h) {
    printHelp();
    return;
  }

  if (command === "doctor") {
    runDoctor();
    return;
  }

  if (command === "list") {
    runList(flags);
    return;
  }

  if (command === "packet") {
    runPacket(flags);
    return;
  }

  throw new Error(`unknown command: ${command}`);
}

function runDoctor() {
  const codex = codexHome();
  const claude = claudeHome();
  const rows = [
    ["codex home", existsLabel(codex), codex],
    ["codex sessions", countLabel(listCodexSessions({ limit: 1000 }).length), path.join(codex, "sessions")],
    ["codex state db", existsLabel(path.join(codex, "state_5.sqlite")), path.join(codex, "state_5.sqlite")],
    ["claude home", existsLabel(claude), claude],
    ["claude projects", existsLabel(path.join(claude, "projects")), path.join(claude, "projects")],
    ["sqlite3", commandExists("sqlite3") ? "available" : "missing", "used for Codex state metadata"],
    ["codex auth", existsLabel(path.join(codex, "auth.json")), "presence only; not read"],
    ["claude credentials", existsLabel(path.join(claude, ".credentials.json")), "presence only; not read"],
  ];
  printTable(["check", "status", "detail"], rows);
}

function runList(flags) {
  const runtime = flags.runtime || flags.from;
  if (!runtime) throw new Error("list requires --runtime codex|claude");
  const limit = Number(flags.limit || 10);
  const rows = listSessions(runtime, limit).map((session) => [
    session.runtime,
    session.sessionId || "",
    session.updatedAt || "",
    session.cwd || "",
    session.gitBranch || "",
    shortCell(session.title || "", 90),
  ]);
  printTable(["runtime", "session", "updated", "cwd", "branch", "title"], rows);
}

function runPacket(flags) {
  const sourceRuntime = flags.from;
  const targetRuntime = flags.to || "unknown";
  if (!sourceRuntime) throw new Error("packet requires --from codex|claude");
  const threadState = readSourceThread(sourceRuntime, flags);
  const packet = compilePacket(threadState, {
    targetRuntime,
    recentLimit: flags.recent ? Number(flags.recent) : 12,
    note: flags.note,
  });
  const outPath = writePacket(packet, {
    root: process.cwd(),
    out: flags.out,
    from: sourceRuntime,
    to: targetRuntime,
  });
  console.log(`wrote ${outPath}`);
  console.log(`source ${sourceRuntime}: ${threadState.sourceSessionId || "unknown"}`);
  console.log(`target ${targetRuntime}`);
}

function listSessions(runtime, limit) {
  if (runtime === "codex") return listCodexSessions({ limit });
  if (runtime === "claude") return listClaudeSessions({ limit });
  throw new Error(`unsupported runtime: ${runtime}`);
}

function readSourceThread(runtime, flags) {
  if (runtime === "codex") {
    if (flags.session) return readCodexSession(flags.session);
    return readCodexLatest();
  }
  if (runtime === "claude") {
    if (flags.session) return readClaudeSession(flags.session);
    return readClaudeLatest();
  }
  throw new Error(`unsupported source runtime: ${runtime}`);
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const flags = {};

  for (let index = 0; index < rest.length; index += 1) {
    const token = rest[index];
    if (!token.startsWith("--")) {
      if (!flags._) flags._ = [];
      flags._.push(token);
      continue;
    }

    const raw = token.slice(2);
    const equalsIndex = raw.indexOf("=");
    if (equalsIndex !== -1) {
      flags[raw.slice(0, equalsIndex)] = raw.slice(equalsIndex + 1);
      continue;
    }

    const next = rest[index + 1];
    if (next && !next.startsWith("--")) {
      flags[raw] = next;
      index += 1;
    } else {
      flags[raw] = true;
    }
  }

  return { command, flags };
}

function printHelp() {
  console.log(`Constant

Usage:
  constant doctor
  constant list --runtime codex
  constant list --runtime claude
  constant packet --from codex --latest --to claude
  constant packet --from claude --latest --to codex

Notes:
  V0 reads runtime homes only. It writes packet artifacts under .constant/packets/.
  --session currently accepts a direct JSONL file path.
  --note adds a manual bridge note when the latest source input was not persisted.
`);
}

function printTable(headers, rows) {
  const widths = headers.map((header, column) =>
    Math.max(
      header.length,
      ...rows.map((row) => String(row[column] == null ? "" : row[column]).length)
    )
  );
  const format = (row) =>
    row
      .map((cell, column) => String(cell == null ? "" : cell).padEnd(widths[column]))
      .join("  ");
  console.log(format(headers));
  console.log(format(headers.map((header) => "-".repeat(header.length))));
  for (const row of rows) console.log(format(row));
}

function existsLabel(filePath) {
  return fs.existsSync(filePath) ? "present" : "missing";
}

function countLabel(count) {
  return count === 1 ? "1 session" : `${count} sessions`;
}

function shortCell(value, maxLength) {
  return cleanText(truncate(String(value || "").replace(/\s+/g, " "), maxLength), maxLength);
}

module.exports = {
  main,
  parseArgs,
};
