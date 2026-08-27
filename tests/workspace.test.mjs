import assert from "node:assert/strict";
import test from "node:test";

import {
  closeWorkspaceRegistry,
  draftEnvironmentChatPanes,
  mergeWorkspaceProfiles,
  normalizeWorkspacePath,
  normalizeWorkspaceExecutionTargetId,
  openWorkspaceRegistry,
  remoteEnvironmentPath,
  setWorkspaceExecutionTarget,
  setWorkspaceMemory,
  terminalEnvironmentPath,
  terminalsForFolder,
  userEnvironmentPath,
  userEnvironmentPathExcluding,
  workspacePathBreadcrumbs,
} from "../src/workspace.ts";

test("les variantes d'un meme chemin Windows ont la meme identite", () => {
  assert.equal(
    normalizeWorkspacePath(" C:\\Projects\\Codex-Switch-Terminal\\ "),
    "c:/projects/codex-switch-terminal",
  );
  assert.equal(
    normalizeWorkspacePath("c:/projects/codex-switch-terminal"),
    "c:/projects/codex-switch-terminal",
  );
});

test("les dossiers identiques fusionnent meme si leurs anciens ids different", () => {
  const merged = mergeWorkspaceProfiles([
    {
      id: "ancien-id-local",
      label: "Codex Switch Terminal",
      path: "C:\\Projects\\Codex-Switch-Terminal\\",
    },
    {
      id: "ancien-id-distant",
      label: "Doublon",
      path: "c:/projects/codex-switch-terminal",
    },
  ]);

  assert.equal(merged.changed, true);
  assert.deepEqual(merged.workspaces, [
    {
      id: "c:/projects/codex-switch-terminal",
      label: "Codex Switch Terminal",
      path: "C:\\Projects\\Codex-Switch-Terminal\\",
      memory: "",
    },
  ]);
});

test("deux chemins Unix dont seule la casse differe restent distincts", () => {
  const merged = mergeWorkspaceProfiles([
    { id: "x", label: "app", path: "/srv/App" },
    { id: "y", label: "app", path: "/srv/app" },
  ]);

  assert.equal(merged.workspaces.length, 2);
});

test("fermer un dossier le retire du registre sans perdre son identite", () => {
  const closed = closeWorkspaceRegistry(
    [{ id: "ancien", label: "Projet", path: "C:\\Projects\\Projet" }],
    [],
    "c:/projects/projet/",
  );

  assert.equal(closed.changed, true);
  assert.deepEqual(closed.workspaces, []);
  assert.deepEqual(closed.closedWorkspaceIds, ["c:/projects/projet"]);
});

test("rouvrir un dossier retire son tombstone et restaure son profil", () => {
  const opened = openWorkspaceRegistry(
    [],
    [" C:\\Projects\\Projet\\ ", "c:/projects/projet"],
    "C:\\Projects\\Projet",
  );

  assert.equal(opened.changed, true);
  assert.deepEqual(opened.closedWorkspaceIds, []);
  assert.deepEqual(opened.workspaces, [
    {
      id: "c:/projects/projet",
      label: "Projet",
      path: "C:\\Projects\\Projet",
      memory: "",
    },
  ]);
});

test("la memoire reste isolee dans son environnement", () => {
  const profiles = [
    { id: "a", label: "Produit", path: "C:\\Projects\\Produit", memory: "" },
    { id: "b", label: "Site", path: "C:\\Projects\\Site", memory: "Ne pas utiliser React." },
  ];

  const updated = setWorkspaceMemory(
    profiles,
    "c:/projects/produit/",
    "  API publique en version 2.\nConserver SQLite.  ",
  );

  assert.equal(updated.changed, true);
  assert.equal(
    updated.workspaces.find((workspace) => workspace.id === "c:/projects/produit")?.memory,
    "API publique en version 2.\nConserver SQLite.",
  );
  assert.equal(
    updated.workspaces.find((workspace) => workspace.id === "c:/projects/site")?.memory,
    "Ne pas utiliser React.",
  );
});

test("le VPS par defaut reste isole dans son environnement", () => {
  const profiles = [
    { id: "a", label: "Produit", path: "C:\\Projects\\Produit", memory: "" },
    { id: "b", label: "Site", path: "C:\\Projects\\Site", memory: "" },
  ];

  const updated = setWorkspaceExecutionTarget(
    profiles,
    "c:/projects/produit/",
    " HTTP://127.0.0.1:18082/ ",
  );

  assert.equal(updated.changed, true);
  assert.equal(
    updated.workspaces.find((workspace) => workspace.id === "c:/projects/produit")?.executionTargetId,
    "http://127.0.0.1:18082",
  );
  assert.equal(
    updated.workspaces.find((workspace) => workspace.id === "c:/projects/site")?.executionTargetId,
    undefined,
  );
  assert.equal(normalizeWorkspaceExecutionTargetId("  "), null);

  const automatic = setWorkspaceExecutionTarget(
    updated.workspaces,
    "C:\\Projects\\Produit",
    null,
  );
  assert.equal(
    automatic.workspaces.find((workspace) => workspace.id === "c:/projects/produit")?.executionTargetId,
    undefined,
  );
});

test("un dossier regroupe plusieurs terminaux dans le meme projet", () => {
  const terminals = [
    {
      key: "terminal-codex",
      agentId: "codex",
      folderPath: "C:\\Projects\\Produit",
      workspaceId: "c:/projects/produit",
      workspacePath: "C:\\Projects\\Produit",
    },
    {
      key: "terminal-claude",
      agentId: "claude",
      folderPath: "c:/projects/produit/",
      workspaceId: "c:/projects/produit",
      workspacePath: "C:\\Projects\\Produit",
    },
    {
      key: "terminal-autre",
      agentId: "codex",
      folderPath: "C:\\Projects\\Autre",
      workspaceId: "c:/projects/autre",
      workspacePath: "C:\\Projects\\Autre",
    },
  ];

  const associated = terminalsForFolder(terminals, "c:/projects/produit");
  assert.deepEqual(associated.map((terminal) => terminal.key), [
    "terminal-codex",
    "terminal-claude",
  ]);
  assert.equal(new Set(associated.map((terminal) => terminal.workspaceId)).size, 1);
});

test("un environnement de terminal doit contenir un dossier explicite", () => {
  assert.equal(terminalEnvironmentPath(undefined), null);
  assert.equal(terminalEnvironmentPath("   "), null);
  assert.equal(
    terminalEnvironmentPath("  C:\\Projects\\Produit  "),
    "C:\\Projects\\Produit",
  );
});

test("un home de compte ne peut pas devenir un environnement projet", () => {
  const accountHome = "%CST_DATA_DIR%\\codex-homes\\compte";
  assert.equal(
    userEnvironmentPathExcluding(
      "%cst_data_dir%/codex-homes/compte/",
      [accountHome],
    ),
    null,
  );
  assert.equal(
    userEnvironmentPathExcluding("/srv/cst/workspaces/produit", [accountHome]),
    "/srv/cst/workspaces/produit",
  );
  assert.equal(
    userEnvironmentPathExcluding("/srv/App", ["/srv/app"]),
    "/srv/App",
  );
});

test("un VPS ignore les chemins locaux Windows", () => {
  assert.equal(remoteEnvironmentPath("C:\\Users\\jeanp\\projet"), null);
  assert.equal(remoteEnvironmentPath("%USERPROFILE%\\projet"), null);
  assert.equal(remoteEnvironmentPath("/srv/cst/workspaces/projet"), "/srv/cst/workspaces/projet");
});

test("le navigateur construit un fil d'Ariane Windows borne a sa racine", () => {
  assert.deepEqual(
    workspacePathBreadcrumbs(
      "C:\\Users\\jeanp",
      "C:\\Users\\jeanp\\codex-switch-terminal\\src",
    ),
    [
      { label: "jeanp", path: "C:\\Users\\jeanp" },
      { label: "codex-switch-terminal", path: "C:\\Users\\jeanp\\codex-switch-terminal" },
      { label: "src", path: "C:\\Users\\jeanp\\codex-switch-terminal\\src" },
    ],
  );
});

test("le navigateur construit aussi un fil d'Ariane Unix", () => {
  assert.deepEqual(workspacePathBreadcrumbs("/srv", "/srv/apps/demo"), [
    { label: "srv", path: "/srv" },
    { label: "apps", path: "/srv/apps" },
    { label: "demo", path: "/srv/apps/demo" },
  ]);
});

test("un chemin choisi est conserve directement comme environnement", () => {
  assert.equal(
    userEnvironmentPath("  C:\\Users\\jeanp\\codex-switch-terminal  "),
    "C:\\Users\\jeanp\\codex-switch-terminal",
  );
});

test("les chats ouverts sans discussion listee restent des brouillons visibles", () => {
  const panes = [
    { key: "pane-neuf", discussion: null }, // nouveau chat, pas encore de rollout
    { key: "pane-liste", discussion: { accountId: "compte-a", sessionId: "s-liste" } }, // deja dans la liste
    { key: "pane-orphelin", discussion: { accountId: "compte-a", sessionId: "s-orphelin" } }, // dossier non resolu
  ];

  const drafts = draftEnvironmentChatPanes(panes, [
    { accountId: "compte-a", sessionId: "s-liste" },
  ]);
  assert.deepEqual(
    drafts.map((pane) => pane.key),
    ["pane-neuf", "pane-orphelin"],
  );
});

test("un chat deja liste n'est jamais duplique en brouillon", () => {
  const panes = [{
    key: "pane-liste",
    discussion: { accountId: "compte-a", sessionId: "s-liste" },
  }];
  assert.deepEqual(draftEnvironmentChatPanes(panes, [
    { accountId: "compte-a", sessionId: "s-liste" },
  ]), []);
});

test("deux comptes gardent leurs panneaux distincts avec le meme identifiant natif", () => {
  const panes = [
    {
      key: "pane-a",
      discussion: { accountId: "freebuff-a", sessionId: "native-id" },
    },
    {
      key: "pane-b",
      discussion: { accountId: "freebuff-b", sessionId: "native-id" },
    },
  ];

  assert.deepEqual(
    draftEnvironmentChatPanes(panes, [
      { accountId: "freebuff-a", sessionId: "native-id" },
    ]).map((pane) => pane.key),
    ["pane-b"],
  );
});
