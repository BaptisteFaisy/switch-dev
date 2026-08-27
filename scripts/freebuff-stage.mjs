#!/usr/bin/env node
// freebuff-stage.mjs <home> <stagingDir>
//
// Prépare une vue "manicode" temporaire que le parseur freebuff de tokscale
// sait lire, à partir des données réellement produites par le CLI :
//   - copie projects/ + projects-archive/ dans <staging>/projects/ ;
//   - injecte le marqueur base2-free (metadata.runState...agentType) que le
//     CLI actuel n'écrit plus, pour que tokscale attribue le chat à freebuff ;
//   - recopie settings.json (freebuffModel) à la racine du canal.
//
// Léger : un fichier à la fois, aucun scan natif, sortie JSON courte.
import fs from "node:fs";
import path from "node:path";

const [, , home, staging] = process.argv;
if (!home || !staging) {
  console.error("usage: freebuff-stage.mjs <home> <staging>");
  process.exit(1);
}

const mani = path.join(home, ".config", "manicode");
const outProjects = path.join(staging, "projects");
fs.mkdirSync(outProjects, { recursive: true });

// settings.json (freebuffModel) → racine du canal simulé
const settingsSrc = path.join(mani, "settings.json");
if (fs.existsSync(settingsSrc)) {
  fs.copyFileSync(settingsSrc, path.join(staging, "settings.json"));
}

const MARKER = {
  runState: { sessionState: { mainAgentState: { agentType: "base2-free" } } },
};

const roots = [
  path.join(mani, "projects"),
  path.join(mani, "projects-archive"),
];

let staged = 0;
for (const root of roots) {
  if (!fs.existsSync(root)) continue;
  for (const project of fs.readdirSync(root)) {
    const chatsDir = path.join(root, project, "chats");
    if (!fs.existsSync(chatsDir)) continue;
    for (const chat of fs.readdirSync(chatsDir)) {
      const src = path.join(chatsDir, chat, "chat-messages.json");
      if (!fs.existsSync(src)) continue;
      const dstDir = path.join(outProjects, project, "chats", chat);
      fs.mkdirSync(dstDir, { recursive: true });
      try {
        const msgs = JSON.parse(fs.readFileSync(src, "utf8"));
        if (Array.isArray(msgs) && msgs.length > 0 && msgs[0] && typeof msgs[0] === "object") {
          msgs[0].metadata = { ...(msgs[0].metadata || {}), ...MARKER };
        }
        fs.writeFileSync(path.join(dstDir, "chat-messages.json"), JSON.stringify(msgs));
        staged += 1;
      } catch {
        // chat illisible → ignoré
      }
    }
  }
}
console.log(JSON.stringify({ staged }));
