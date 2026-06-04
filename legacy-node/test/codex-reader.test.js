const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { readCodexSession } = require("../src/runtimes/codex");
const { compilePacket } = require("../src/packet");

test("Codex reader extracts neutral turns and redacts secrets", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "constant-test-"));
  const filePath = path.join(
    tempDir,
    "rollout-2026-01-02T03-04-05-abc123.jsonl"
  );
  const rows = [
    {
      type: "session_meta",
      timestamp: "2026-01-02T03:04:05.000Z",
      payload: {
        id: "abc123",
        cwd: tempDir,
        cli_version: "0.test",
        model_provider: "OpenAI",
        source: "cli",
      },
    },
    {
      type: "turn_context",
      timestamp: "2026-01-02T03:04:06.000Z",
      payload: {
        cwd: tempDir,
        model: "gpt-test",
        summary: "Earlier summary.",
      },
    },
    {
      type: "response_item",
      timestamp: "2026-01-02T03:04:07.000Z",
      payload: {
        type: "message",
        role: "user",
        content: [{ type: "input_text", text: "Build a packet. token=supersecret daniel@example.com" }],
      },
    },
    {
      type: "response_item",
      timestamp: "2026-01-02T03:04:08.000Z",
      payload: {
        type: "function_call",
        name: "exec_command",
        call_id: "call_1",
        arguments: JSON.stringify({ cmd: "git status --short", workdir: tempDir }),
      },
    },
    {
      type: "response_item",
      timestamp: "2026-01-02T03:04:09.000Z",
      payload: {
        type: "message",
        role: "assistant",
        content: [{ type: "output_text", text: "Packet compiler exists." }],
      },
    },
  ];
  fs.writeFileSync(filePath, `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`);

  const state = readCodexSession(filePath);
  assert.equal(state.sourceRuntime, "codex");
  assert.equal(state.sourceSessionId, "abc123");
  assert.equal(state.cwd, tempDir);
  assert.equal(state.model, "gpt-test");
  assert.equal(state.events.length, 3);
  assert.equal(state.events[0].role, "user");
  assert.match(state.events[0].text, /token=\[redacted\]/);
  assert.doesNotMatch(state.events[0].text, /daniel@example.com/);

  const packet = compilePacket(state, { targetRuntime: "claude", note: "Manual note." });
  assert.match(packet, /# Constant Continuation Packet/);
  assert.match(packet, /Earlier summary/);
  assert.match(packet, /Manual Bridge Note/);
  assert.match(packet, /Packet compiler exists/);
  assert.doesNotMatch(packet, /supersecret/);
});
