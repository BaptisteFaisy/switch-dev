import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  filterPrivateMessageUsers,
  privateMessageCampaignCounts,
  privateMessagingUnreadCount,
  renderPrivateMessageCampaignTemplate,
  sortPrivateConversations,
} from "../src/messaging-model.ts";

const backend = readFileSync(new URL("../src-tauri/src/private_messages.rs", import.meta.url), "utf8");
const auth = readFileSync(new URL("../src-tauri/src/auth.rs", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");
const desktop = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8");
const messaging = readFileSync(new URL("../src/messaging.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/messaging.css", import.meta.url), "utf8");

const user = (id, username) => ({ id, username, avatarUrl: null });
const message = (id, sequence, sender, recipient, body = "Message") => ({
  id,
  sender,
  recipient,
  body,
  images: [],
  createdAt: sequence,
  sequence,
  readAt: null,
});

test("la recherche de destinataires ignore les accents et conserve un tri stable", () => {
  const users = [user("z", "Zoé"), user("e", "Émile"), user("a", "Alice")];
  assert.deepEqual(
    filterPrivateMessageUsers(users, "emil").map((entry) => entry.id),
    ["e"],
  );
  assert.deepEqual(
    filterPrivateMessageUsers(users, "").map((entry) => entry.id),
    ["a", "e", "z"],
  );
});

test("les conversations sont classees par dernier message et additionnent les non-lus", () => {
  const me = user("me", "Moi");
  const alice = user("alice", "Alice");
  const zoe = user("zoe", "Zoé");
  const conversations = [
    { user: alice, lastMessage: message("1", 3, alice, me), unreadCount: 2 },
    { user: zoe, lastMessage: message("2", 8, me, zoe), unreadCount: 1 },
  ];
  assert.deepEqual(sortPrivateConversations(conversations).map((entry) => entry.user.id), ["zoe", "alice"]);
  assert.equal(privateMessagingUnreadCount(conversations), 3);
});

test("les campagnes personnalisent les variantes et comptent leur progression", () => {
  const alice = user("alice", "Alice");
  assert.equal(
    renderPrivateMessageCampaignTemplate(
      "Bonjour {{username}} ({{index}}/{{total}})",
      alice,
      2,
      5,
    ),
    "Bonjour Alice (2/5)",
  );
  assert.deepEqual(privateMessageCampaignCounts({
    deliveries: [
      { status: "sent" },
      { status: "queued" },
      { status: "failed" },
    ],
  }), { sent: 1, failed: 1, queued: 1, total: 3 });
});

test("le stockage filtre chaque fil par ses deux participants et persiste les lectures", () => {
  assert.match(backend, /filter\(\|message\| message_involves\(message, viewer_id\)\)/);
  assert.match(backend, /filter\(\|message\| message_between\(message, &viewer_id, &other_id\)\)/);
  assert.match(backend, /message\.recipient\.id == viewer_id && message\.read_at\.is_none\(\)/);
  assert.match(backend, /let mut current = self\.lock_store\(\)\?;[\s\S]*?let mut next = current\.clone\(\);/);
  assert.match(backend, /persist_store\(&self\.inner\.storage_path, &next\)\?;[\s\S]*?\*current = next;/);
  assert.match(backend, /MAX_MESSAGE_CHARS: usize = 4_000/);
  assert.match(backend, /fs_util::atomic_write\(path, content\)/);
});

test("les API derivent toujours l'expediteur de la session et n'exposent pas les e-mails", () => {
  assert.match(auth, /pub\(crate\) fn public_identities/);
  assert.match(auth, /AuthIdentity \{[\s\S]*?id: user\.id\.clone\(\),[\s\S]*?username: user\.username\.clone\(\),[\s\S]*?avatar_url: user\.avatar_url\.clone\(\)/);
  assert.match(server, /"\/private-messages\/users"/);
  assert.match(server, /"\/private-messages\/conversations\/:user_id"/);
  assert.match(server, /fn private_message_actor\([\s\S]*?identity_from_headers\(headers\)/);
  assert.match(server, /known_conversation_participant\(&actor\.id, user_id\)/);
  assert.match(server, /fn private_message_no_store[\s\S]*?HeaderValue::from_static\("no-store"\)/);
  assert.match(platform, /case "send_private_message"[\s\S]*?\/api\/private-messages\/conversations/);
});

test("l'onglet Messagerie et son icone enveloppe existent sur desktop et mobile", () => {
  assert.match(types, /\| "messaging"/);
  assert.match(main, /id="messagingToggle"[\s\S]*?data-lucide="mail"[\s\S]*?<span>Messagerie<\/span>/);
  assert.match(main, /class="m-tab" type="button" data-view="messaging"[\s\S]*?data-lucide="mail"[\s\S]*?<span>Messages<\/span>/);
  assert.match(main, /type MessagingModule = typeof import\("\.\/messaging"\)/);
  assert.match(main, /messagingModulePromise = import\("\.\/messaging"\)/);
  assert.match(main, /if \(view === "messaging" && !messagingModule\)/);
  assert.match(main, /case "messaging":\s*return messagingModule\?\.renderMessagingPanel\(\) \?\? "";/);
  assert.doesNotMatch(main, /import "\.\/messaging\.css";/);
});

test("la vue permet de choisir un utilisateur, lire un fil et envoyer un message", () => {
  assert.match(messaging, /id="messagingPanel"/);
  assert.match(messaging, /data-private-message-user-id/);
  assert.match(messaging, /id="privateMessageForm"/);
  assert.match(messaging, /invoke<PrivateMessage>\("send_private_message"/);
  assert.match(messaging, /threadIsVisible = messagingVisible[\s\S]*?messagingMobileDetailOpen/);
  assert.match(main, /setMessagingVisible\(activeView === "messaging"\)/);
  assert.match(main, /data-messaging-nav-count/);
  assert.match(desktop, /private_messages::send_private_message/);
  assert.match(styles, /@media \(max-width: 860px\)/);
  assert.match(styles, /\.messaging-panel\.is-detail-open \.messaging-list \{ display: none;/);
  assert.match(styles, /\.messaging-panel\.is-detail-open \.messaging-detail \{ display: flex;/);
});

test("les campagnes de messages sont pilotees depuis l'interface et les deux transports", () => {
  assert.match(messaging, /id="messagingCampaigns"/);
  assert.match(messaging, /id="messagingCampaignForm"/);
  assert.match(messaging, /data-campaign-recipient/);
  assert.match(messaging, /renderPrivateMessageCampaignTemplate/);
  assert.match(messaging, /invoke<PrivateMessageCampaign>\(\s*"create_private_message_campaign"/);
  assert.match(messaging, /"control_private_message_campaign"/);
  assert.match(platform, /case "list_private_message_campaigns"/);
  assert.match(platform, /case "create_private_message_campaign"/);
  assert.match(platform, /case "control_private_message_campaign"/);
  assert.match(desktop, /private_messages::create_private_message_campaign/);
  assert.match(server, /"\/private-messages\/campaigns"/);
  assert.match(backend, /pub fn process_due_campaigns/);
  assert.match(backend, /MIN_CAMPAIGN_INTERVAL_SECONDS: u64 = 5/);
  assert.match(styles, /\.messaging-campaign-dialog/);
});

test("la messagerie accepte, valide, affiche et protege les images privees", () => {
  assert.match(messaging, /id="privateMessageImages"[^>]*accept="image\/png,image\/jpeg,image\/webp"[^>]*multiple/);
  assert.match(messaging, /readChatImageAttachments\(files, messagingImageAttachments\)/);
  assert.match(messaging, /clipboardChatImageFiles\(event\.clipboardData\)/);
  assert.match(messaging, /data-messaging-remove-image/);
  assert.match(messaging, /chatImageAttachmentPayloads\(submittedImages\)/);
  assert.match(messaging, /invoke<PrivateMessageImageContent>\("get_private_message_image"/);
  assert.match(messaging, /new IntersectionObserver/);
  assert.match(messaging, /feed\.scrollHeight - feed\.scrollTop - feed\.clientHeight <= 24/);
  assert.match(messaging, /data-messaging-open-image/);
  assert.match(styles, /\.messaging-draft-images/);
  assert.match(styles, /\.messaging-message-images/);
  assert.match(styles, /\.messaging-image-viewer/);

  assert.match(backend, /MAX_MESSAGE_IMAGES: usize = 4/);
  assert.match(backend, /MAX_MESSAGE_IMAGE_BYTES: usize = 8 \* 1024 \* 1024/);
  assert.match(backend, /validated_private_message_image_extension/);
  assert.match(backend, /filter\(\|message\| message_involves\(message, viewer_id\)\)/);
  assert.match(backend, /pub fn image_content/);
  assert.match(server, /"\/private-messages\/images\/:image_id"/);
  assert.match(server, /MAX_PRIVATE_MESSAGE_REQUEST_BYTES/);
  assert.match(platform, /case "get_private_message_image"/);
  assert.match(desktop, /private_messages::get_private_message_image/);
});

test("le temps reel prive coupe les deux polls sans retirer leur repli", () => {
  assert.match(platform, /RuntimeSyncTopic =[^;]*"privateMessages"/);
  assert.match(main, /queueRuntimeSyncUpdate\("privateMessages"\)/);
  assert.match(main, /module\.refreshMessaging\(render, \{ silent: true \}\)/);
  assert.match(main, /return 8_000/);
  assert.match(messaging, /setMessagingRealtimeAvailable/);
  assert.match(
    messaging,
    /messagingRealtimeAvailable \|\| !messagingPollRerender[\s\S]*?clearMessagingPollTimer\(\)/,
  );
  assert.match(
    messaging,
    /window\.setInterval\([\s\S]*?refreshMessaging\(messagingPollRerender, \{ silent: true \}\)[\s\S]*?MESSAGING_POLL_INTERVAL_MS/,
  );
  assert.match(auth, /notify\(RuntimeSyncTopic::PrivateMessages\)/);
  assert.match(backend, /conversation_with_read_status/);
  assert.match(server, /notify_private_messages\(\[left\.id\.clone\(\), right\.id\.clone\(\)\]\)/);
});
