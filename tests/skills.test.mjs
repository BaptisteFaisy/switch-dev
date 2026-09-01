import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  CUSTOM_SKILLS_STORAGE_KEY,
  CUSTOM_SKILL_LIMIT,
  SKILL_CONTENT_MAX_BYTES,
  createCustomSkillId,
  loadCustomSkills,
  normalizeCustomSkills,
  persistCustomSkills,
  removeCustomSkill,
  saveCustomSkill,
  skillContentBytes,
  skillDraftFromMarkdown,
} from "../src/skills.ts";

const memoryStorage = () => {
  const values = new Map();
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    values,
  };
};

test("normalise les skills personnels et ignore les entrées dangereuses ou invalides", () => {
  const skills = normalizeCustomSkills([
    {
      id: "custom-review",
      name: "  Revue   exigeante ",
      description: "  À utiliser avant chaque livraison.  ",
      tags: ["Code", " code ", "Qualité"],
      buttonLabel: "  Revue ",
      icon: "Shield-Check",
      content: "\r\n# Instructions\r\n\r\nAnalyse le code.\r\n",
      createdAt: 10,
      updatedAt: 20,
    },
    { id: "custom-review", name: "Doublon", content: "Ignoré" },
    { id: "<script>", name: "Invalide", content: "Ignoré" },
    { id: "custom-empty", name: "Vide", content: "   " },
  ], 99);

  assert.deepEqual(skills, [{
    id: "custom-review",
    name: "Revue exigeante",
    description: "À utiliser avant chaque livraison.",
    tags: ["Code", "Qualité"],
    buttonLabel: "Revue",
    icon: "shield-check",
    content: "# Instructions\n\nAnalyse le code.",
    custom: true,
    createdAt: 10,
    updatedAt: 20,
  }]);
});

test("crée un identifiant lisible sans collision avec les skills existants", () => {
  assert.equal(createCustomSkillId("Révision sécurité"), "custom-revision-securite");
  assert.equal(
    createCustomSkillId("Révision sécurité", ["custom-revision-securite"]),
    "custom-revision-securite-2",
  );
});

test("ajoute et modifie un skill sans changer son identifiant", () => {
  const created = saveCustomSkill([], {
    name: "Revue sécurité",
    description: "Pour les changements sensibles.",
    tags: "sécurité, code, sécurité",
    content: "# Rôle\n\nCherche les vulnérabilités.",
  }, {
    reservedIds: ["custom-revue-securite"],
    timestamp: 1_000,
  });
  assert.equal(created.ok, true);
  if (!created.ok) return;
  assert.equal(created.skill.id, "custom-revue-securite-2");
  assert.deepEqual(created.skill.tags, ["sécurité", "code"]);
  assert.equal(created.created, true);

  const updated = saveCustomSkill(created.items, {
    name: "Revue sécurité renforcée",
    content: "# Rôle\n\nCherche puis corrige les vulnérabilités.",
  }, {
    id: created.skill.id,
    timestamp: 2_000,
  });
  assert.equal(updated.ok, true);
  if (!updated.ok) return;
  assert.equal(updated.skill.id, created.skill.id);
  assert.equal(updated.skill.createdAt, 1_000);
  assert.equal(updated.skill.updatedAt, 2_000);
  assert.equal(updated.created, false);
  assert.deepEqual(removeCustomSkill(updated.items, updated.skill.id), []);
});

test("refuse un contenu qui dépasse la limite UTF-8 et borne le nombre de skills", () => {
  const oversized = "é".repeat(Math.floor(SKILL_CONTENT_MAX_BYTES / 2) + 1);
  assert.ok(skillContentBytes(oversized) > SKILL_CONTENT_MAX_BYTES);
  const rejected = saveCustomSkill([], { name: "Trop grand", content: oversized });
  assert.deepEqual(rejected, { ok: false, error: "Le contenu dépasse 64 Ko." });

  const full = Array.from({ length: CUSTOM_SKILL_LIMIT }, (_, index) => ({
    id: `custom-skill-${index}`,
    name: `Skill ${index}`,
    description: "",
    tags: [],
    buttonLabel: `Skill ${index}`,
    icon: "sparkles",
    content: `Instruction ${index}`,
    custom: true,
    createdAt: index,
    updatedAt: index,
  }));
  const atCapacity = saveCustomSkill(full, { name: "Un de trop", content: "Instructions" });
  assert.deepEqual(atCapacity, {
    ok: false,
    error: `La limite de ${CUSTOM_SKILL_LIMIT} skills personnels est atteinte.`,
  });
});

test("persiste la bibliothèque et résiste à un stockage corrompu", () => {
  const storage = memoryStorage();
  const result = saveCustomSkill([], {
    name: "Skill persistant",
    content: "Toujours disponible après le redémarrage.",
  }, { timestamp: 3_000 });
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(persistCustomSkills(result.items, storage), true);
  assert.deepEqual(loadCustomSkills(storage), result.items);
  assert.ok(storage.values.has(CUSTOM_SKILLS_STORAGE_KEY));

  storage.values.set(CUSTOM_SKILLS_STORAGE_KEY, "{json-invalide");
  assert.deepEqual(loadCustomSkills(storage), []);
});

test("importe un SKILL.md standard et retire son frontmatter des instructions", () => {
  const imported = skillDraftFromMarkdown("SKILL.md", `---
name: security-review
description: >
  Use when reviewing authentication
  or other sensitive code.
tags: [security, "code review", security]
---
# Security review

Inspect every trust boundary before proposing a fix.
`);

  assert.deepEqual(imported, {
    name: "security-review",
    description: "Use when reviewing authentication or other sensitive code.",
    tags: ["security", "code review"],
    content: "# Security review\n\nInspect every trust boundary before proposing a fix.",
  });
  assert.doesNotMatch(imported.content, /description:|^---/);
});

test("déduit le nom et la description d’un Markdown sans frontmatter", () => {
  assert.deepEqual(skillDraftFromMarkdown("quality-check.md", `# Contrôle qualité

Vérifie les tests, les types et les erreurs silencieuses.

## Étapes

1. Lire le diff.
`), {
    name: "Contrôle qualité",
    description: "Vérifie les tests, les types et les erreurs silencieuses.",
    tags: [],
    content: "# Contrôle qualité\n\nVérifie les tests, les types et les erreurs silencieuses.\n\n## Étapes\n\n1. Lire le diff.",
  });
});

test("la vue Skills expose création, import, édition et suppression", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
  const skills = readFileSync(new URL("../src/skills.ts", import.meta.url), "utf8");
  const view = readFileSync(new URL("../src/skills-view.ts", import.meta.url), "utf8");
  const viewStyle = readFileSync(new URL("../src/skills-view.css", import.meta.url), "utf8");

  assert.match(main, /import\("\.\/skills-view"\)/);
  assert.match(main, /loadCustomSkills\(accountScopedStorage\)/);
  assert.match(main, /Skill « \$\{skill\.name\} » ajouté et disponible dans les chats/);
  assert.match(view, /id="skillsAdd"/);
  assert.match(view, /data-skill-edit/);
  assert.match(view, /data-skill-delete/);
  assert.match(view, /id="skillImportInput"/);
  assert.match(view, /dataTransfer\?\.files\[0\]/);
  assert.match(view, /openCustomSkillEditor/);
  assert.match(view, /import "\.\/skills-view\.css"/);
  assert.doesNotMatch(skills, /openCustomSkillEditor/);
  assert.doesNotMatch(style, /\.skill(?:s-list|-editor-dialog)/);
  assert.match(viewStyle, /\.skill-editor-dialog::backdrop/);
  assert.match(viewStyle, /@media \(max-width: 700px\)[\s\S]*?\.skill-editor-grid/);
});

test("le skill TikTok suit les contrats Freebuff et MCP réellement exposés", () => {
  const manifest = JSON.parse(readFileSync(
    new URL("../public/skills/index.json", import.meta.url),
    "utf8",
  ));
  const entry = manifest.skills.find(({ id }) => id === "switch-tiktok-assisted-navigation");
  assert.ok(entry, "le skill doit être déclaré sous son identifiant stable");
  assert.equal(entry.buttonLabel, "Utilisation humaine");
  const assistedNavigationSource = readFileSync(
    new URL("../src/tiktok-assisted-navigation.ts", import.meta.url),
    "utf8",
  );
  assert.match(assistedNavigationSource, />Utilisation humaine<\/strong>/);
  const content = readFileSync(
    new URL(`../public/skills/${entry.file}`, import.meta.url),
    "utf8",
  );

  assert.match(content, /^---\s*\nname: switch-tiktok-assisted-navigation\s*$/m);
  assert.match(content, /CST_DEVICE_API_URL/);
  assert.match(content, /CST_DEVICE_TOKEN/);
  assert.match(content, /CST_DEVICE_HELPER=cst-device/);
  assert.match(content, /CST_SERVER_BIN/);
  assert.match(content, /exact path `\/api\/device-fleet`/);
  assert.match(content, /no username, password, query, or fragment/);
  assert.match(content, /cst-device list/);
  assert.match(content, /cst-device action DEVICE_ID ACTION \[ARGS_JSON\] \[--confirm\]/);
  assert.match(content, /cst-device status ACTION_ID/);
  assert.match(content, /list_control_devices/);
  assert.match(content, /control_device/);
  assert.match(content, /get_control_device_action/);
  for (const action of [
    "info",
    "screenshot",
    "open_screen",
    "tap",
    "swipe",
    "type_text",
    "key_event",
    "open_app",
    "shell",
  ]) {
    assert.match(content, new RegExp(`\\b${action}\\b`));
  }
  assert.match(content, /this TikTok skill must never invoke it/);
  assert.match(content, /Never choose content to like[\s\S]*autonomously/);
  assert.match(content, /minimal in-memory action journal/);
  assert.match(content, /opaque random alias/);
  assert.doesNotMatch(content, /CST_USB_/);
  assert.doesNotMatch(content, /\b(?:input-text|launch-app|open-screen|stop-app)\b/);
  assert.doesNotMatch(content, /node\s+["']?\$env?:?CST_/i);
});

test("le skill Publier vidéo couvre un flux USB borné sur les trois plateformes", () => {
  const manifest = JSON.parse(readFileSync(
    new URL("../public/skills/index.json", import.meta.url),
    "utf8",
  ));
  const entry = manifest.skills.find(({ id }) => id === "publier-video-reseaux");
  assert.ok(entry, "le skill doit être déclaré sous son identifiant stable");
  assert.equal(entry.buttonLabel, "Publier vidéo");
  const content = readFileSync(
    new URL(`../public/skills/${entry.file}`, import.meta.url),
    "utf8",
  );

  assert.match(content, /^---\s*\nname: publier-video-reseaux\s*$/m);
  assert.match(content, /CST_DEVICE_API_URL/);
  assert.match(content, /CST_DEVICE_TOKEN/);
  assert.match(content, /CST_DEVICE_HELPER=cst-device/);
  assert.match(content, /CST_SERVER_BIN/);
  assert.match(content, /(?:exact path|chemin exact) `\/api\/device-fleet`/);
  assert.match(content, /cst-device list/);
  assert.match(content, /cst-device action DEVICE_ID ACTION \[ARGS_JSON\] \[--confirm\]/);
  assert.match(content, /cst-device status ACTION_ID/);
  assert.match(content, /list_control_devices/);
  assert.match(content, /control_device/);
  assert.match(content, /get_control_device_action/);
  for (const action of [
    "info",
    "screenshot",
    "open_screen",
    "tap",
    "swipe",
    "type_text",
    "key_event",
    "open_app",
    "shell",
    "push_file",
  ]) {
    assert.match(content, new RegExp(`\\b${action}\\b`));
  }
  for (const platform of [
    "TikTok",
    "Instagram",
    "YouTube",
    "com.zhiliaoapp.musically",
    "com.instagram.android",
    "com.google.android.youtube",
    "com.google.android.apps.youtube.creator",
  ]) {
    assert.match(content, new RegExp(platform));
  }
  assert.match(content, /localPath/);
  assert.match(content, /remotePath/);
  assert.match(content, /\/sdcard\/Pictures|\/sdcard\/Download/);
  assert.match(content, /curl -L/);
  assert.match(content, /Ce skill ne doit jamais invoquer `shell`/);
  assert.match(content, /jamais deviner/);
  assert.match(content, /ne s[eé]lectionne jamais un compte/);
  assert.match(content, /Publier|Partager|T[eé]l[eé]verser/);
  assert.match(content, /CAPTCHA/);
  assert.match(content, /journal d'actions minimal en m[eé]moire/);
  assert.match(content, /alias al[eé]atoire opaque/);
  assert.doesNotMatch(content, /\b(?:input-text|launch-app|open-screen|stop-app)\b/);
  assert.doesNotMatch(content, /node\s+["']?\$env?:?CST_/i);
});
