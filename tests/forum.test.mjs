import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { sortForumTopics } from "../src/forum-model.ts";

const backend = readFileSync(new URL("../src-tauri/src/forum.rs", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");
const desktop = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8");
const forum = readFileSync(new URL("../src/forum.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/forum.css", import.meta.url), "utf8");
const runtimeHarness = readFileSync(new URL("../scripts/test-forum-runtime.mjs", import.meta.url), "utf8");

const topic = (id, activitySequence, lastActivityAt = activitySequence) => ({
  id,
  title: `Sujet ${id}`,
  excerpt: "Message",
  author: { id: "author", username: "Auteur" },
  createdAt: 1,
  lastActivityAt,
  activitySequence,
  replyCount: 0,
  lastReplyAuthor: null,
});

test("une nouvelle activite place toujours le sujet en tete", () => {
  const sorted = sortForumTopics([
    topic("ancien", 4, 100),
    topic("repondu", 7, 100),
    topic("intermediaire", 5, 100),
  ]);
  assert.deepEqual(sorted.map((entry) => entry.id), ["repondu", "intermediaire", "ancien"]);
});

test("le backend persiste les sujets et attribue une sequence a chaque reponse", () => {
  assert.match(backend, /next_activity_sequence: u64/);
  assert.match(backend, /let activity_sequence = next_activity_sequence\(store\);[\s\S]*?topic\.replies\.push/);
  assert.match(backend, /topic\.activity_sequence = activity_sequence/);
  assert.match(backend, /right\s*\.activity_sequence\s*\.cmp\(&left\.activity_sequence\)/);
  assert.match(backend, /fs_util::atomic_write\(path, content\)/);
});

test("les routes serveur et desktop couvrent liste, creation, lecture et reponse", () => {
  assert.match(server, /"\/forum\/topics",\s*get\(api_list_forum_topics\)\.post\(api_create_forum_topic\)/);
  assert.match(server, /"\/forum\/topics\/:id\/replies",\s*post\(api_reply_to_forum_topic\)/);
  assert.match(server, /identity_from_headers\(headers\)/);
  assert.match(desktop, /forum::list_forum_topics/);
  assert.match(desktop, /forum::reply_to_forum_topic/);
  assert.match(platform, /case "list_forum_topics"/);
  assert.match(platform, /case "reply_to_forum_topic"/);
  assert.match(server, /fn forum_no_store/);
  assert.match(runtimeHarness, /\/api\/forum\/topics/);
  assert.match(runtimeHarness, /cache-control/);
  assert.match(runtimeHarness, /forum\.json/);
});

test("le nouvel onglet Forum est present sur desktop et mobile", () => {
  assert.match(types, /\| "forum"/);
  assert.match(main, /id="forumToggle"/);
  assert.match(main, /class="m-tab" type="button" data-view="forum"/);
  assert.match(main, /type ForumModule = typeof import\("\.\/forum"\);/);
  assert.match(main, /forumModulePromise = import\("\.\/forum"\)/);
  assert.match(main, /if \(view === "forum" && !forumModule\)/);
  assert.match(main, /case "forum":\s*return forumModule\?\.renderForumPanel\(\) \?\? "";/);
  assert.match(main, /forumModule\?\.startForumPolling\(render\)/);
  assert.match(main, /forumModule\?\.openForumComposer\(render\)/);
  assert.match(main, /forumModule\?\.bindForumUi\(\{/);
  assert.doesNotMatch(main, /import "\.\/forum\.css";/);
  assert.match(forum, /import "\.\/forum\.css";/);
});

test("le panneau explique la remontee et reste adapte aux petits ecrans", () => {
  assert.match(forum, /id="forumPanel"/);
  assert.match(forum, /Triés par dernière activité/);
  assert.match(forum, /data-forum-new-topic/);
  assert.match(styles, /@media \(max-width: 860px\)/);
  assert.match(styles, /\.forum-panel\.is-detail-open \.forum-topic-list \{ display: none;/);
  assert.match(styles, /\.forum-panel\.is-detail-open \.forum-detail \{ display: flex;/);
});
