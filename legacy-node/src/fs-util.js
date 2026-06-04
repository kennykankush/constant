const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

function expandHome(value) {
  if (!value) return value;
  if (value === "~") return os.homedir();
  if (value.startsWith("~/")) return path.join(os.homedir(), value.slice(2));
  return value;
}

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

function readJsonLines(filePath) {
  const text = fs.readFileSync(filePath, "utf8");
  return text
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line, index) => {
      try {
        return JSON.parse(line);
      } catch (error) {
        const message = `${filePath}:${index + 1}: invalid JSONL record`;
        throw new Error(`${message}: ${error.message}`);
      }
    });
}

function walkFiles(root, predicate) {
  if (!fs.existsSync(root)) return [];
  const results = [];
  const stack = [root];

  while (stack.length > 0) {
    const current = stack.pop();
    let entries = [];
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }

    for (const entry of entries) {
      const fullPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(fullPath);
      } else if (entry.isFile() && (!predicate || predicate(fullPath))) {
        results.push(fullPath);
      }
    }
  }

  return results;
}

function statMtimeMs(filePath) {
  try {
    return fs.statSync(filePath).mtimeMs;
  } catch {
    return 0;
  }
}

function commandExists(command) {
  try {
    execFileSync("sh", ["-lc", `command -v ${shellQuote(command)}`], {
      stdio: "ignore",
    });
    return true;
  } catch {
    return false;
  }
}

function shellQuote(value) {
  return `'${String(value).replace(/'/g, "'\\''")}'`;
}

function tryGitState(cwd) {
  if (!cwd || !fs.existsSync(cwd)) {
    return { branch: undefined, changedFiles: [], isGitRepo: false };
  }

  try {
    const branch = execFileSync("git", ["-C", cwd, "branch", "--show-current"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    const status = execFileSync("git", ["-C", cwd, "status", "--short"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    const changedFiles = status
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => line.trim());
    return { branch: branch || undefined, changedFiles, isGitRepo: true };
  } catch {
    return { branch: undefined, changedFiles: [], isGitRepo: false };
  }
}

module.exports = {
  commandExists,
  ensureDir,
  expandHome,
  readJsonLines,
  shellQuote,
  statMtimeMs,
  tryGitState,
  walkFiles,
};
