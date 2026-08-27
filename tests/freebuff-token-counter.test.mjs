import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";

const root = path.join(os.tmpdir(), `cst-freebuff-counter-${process.pid}`);
const home = path.join(root, "freebuff-test", ".config", "manicode", "projects", "demo", "chats", "session-1");
fs.mkdirSync(home, { recursive: true });
fs.writeFileSync(path.join(root, "freebuff-test", ".config", "manicode", "settings.json"), JSON.stringify({ freebuffModel: "deepseek/deepseek-v4-flash" }));
fs.writeFileSync(path.join(home, "chat-messages.json"), JSON.stringify([
  { variant: "user", content: "a".repeat(40) },
  { variant: "assistant", blocks: [{ type: "text", content: "b".repeat(20) }] },
]));

test("Freebuff counter reports transcript usage and does not fabricate cache", () => {
  const script = new URL("../scripts/freebuff-estimate.mjs", import.meta.url);
  const scriptPath = process.platform === "win32"
    ? script.pathname.replace(/^\/(?:[A-Za-z]:)/, (value) => value.slice(1)).replaceAll("/", "\\\\")
    : script.pathname;
  const output = execFileSync(process.execPath, [scriptPath, root], { encoding: "utf8" });
  const [usage] = JSON.parse(output);
  assert.equal(usage.input, 10);
  assert.equal(usage.output, 5);
  assert.equal(usage.cacheRead, 0);
  assert.equal(usage.cacheWrite, 0);
  assert.equal(usage.cacheConfidence, "unavailable");
  assert.equal(usage.inputConfidence, "estimated_transcript");
  assert.equal(usage.counterVersion, 2);
});

test.after(() => fs.rmSync(root, { recursive: true, force: true }));
