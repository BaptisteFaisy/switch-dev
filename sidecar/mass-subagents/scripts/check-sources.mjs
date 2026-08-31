import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";

const root = path.resolve(import.meta.dirname, "..");

const walk = async (directory) => {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (["data", "artifacts", "chat-factory", "node_modules"].includes(entry.name)) continue;
    const target = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await walk(target));
    else if (entry.name.endsWith(".mjs")) files.push(target);
  }
  return files;
};

const files = await walk(root);
for (const file of files) {
  const source = await readFile(file, "utf8");
  if (source.includes("\t")) {
    throw new Error(`${path.relative(root, file)} contains a tab character`);
  }
  const result = spawnSync(process.execPath, ["--check", file], { encoding: "utf8" });
  if (result.status !== 0) {
    process.stderr.write(`syntax check failed for ${path.relative(root, file)}\n`);
    if (result.error) process.stderr.write(`${result.error.message}\n`);
    if (result.stdout) process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    process.exit(result.status ?? 1);
  }
}

process.stdout.write(`checked ${files.length} module(s)\n`);
