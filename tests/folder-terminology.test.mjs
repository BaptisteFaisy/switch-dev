import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const chatView = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");

test("l'interface presente les projets comme des environnements", () => {
  for (const label of [
    "Choisir un environnement",
    "Choisir l'environnement actif",
    "Environnement de ce terminal",
  ]) {
    assert.ok(main.includes(label), `libelle manquant: ${label}`);
  }

  assert.match(chatView, /explore l'environnement/);
  assert.match(chatView, /travaille dans l'environnement/);
});

test("les anciens libelles workspace ne reapparaissent pas dans l'interface", () => {
  const visibleSources = `${main}\n${chatView}`;
  for (const oldLabel of [
    "<strong>Workspaces</strong>",
    "Workspace de ce terminal",
    "Choisir le workspace actif",
    "Aucun workspace actif",
    "Workspace actif:",
    "Choisir ou ajouter un workspace",
    "Fermer le workspace",
    "travaille dans le workspace",
    "explore le workspace",
    "<strong>Dossiers</strong>",
    "Choisir le dossier actif",
    "Dossier de ce terminal",
    "explore le dossier",
    "travaille dans le dossier",
  ]) {
    assert.equal(visibleSources.includes(oldLabel), false, `ancien libelle present: ${oldLabel}`);
  }
});

test("un terminal reste rattache a l'environnement actif", () => {
  for (const marker of [
    "openFolderTerminals",
    "data-open-folder-terminal",
    "folder-terminal-panel",
    "Dossier en preparation",
    "Environnement actif",
  ]) {
    assert.ok(main.includes(marker), `parcours environnement/terminal incomplet: ${marker}`);
  }
  assert.match(platform, /return value as T/);
  assert.doesNotMatch(platform, /return response\.id as T/);
});
