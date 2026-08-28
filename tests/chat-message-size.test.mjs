import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  CHAT_BUBBLE_MAX_CHARS,
  CHAT_COMPOSER_MAX_CHARS,
  chatBubbleText,
  resumeSeedBubbleText,
} from "../src/chat/message-size.ts";

test("une bulle de taille normale n'est pas touchee", () => {
  assert.equal(chatBubbleText("corrige le bug"), "corrige le bug");
  assert.equal(chatBubbleText(""), "");
});

test("une bulle demesuree est tronquee et annonce ce qu'elle masque", () => {
  const prompt = "a".repeat(CHAT_BUBBLE_MAX_CHARS + 4_321);
  const bubble = chatBubbleText(prompt);

  assert.ok(bubble.length < prompt.length, "la bulle est bien plus courte");
  assert.match(bubble, /4[  ]?321 caracteres masques/);
  assert.match(bubble, /Le modele a recu le texte complet/);
});

test("un prompt engendre par l'application affiche son resume, pas son contenu", () => {
  const seed = "UTILISATEUR: ...\n\n".repeat(5_000);
  const summary = resumeSeedBubbleText(seed);

  assert.equal(chatBubbleText(seed, summary), summary);
  assert.ok(summary.length < 200, "le resume tient en une ligne");
  assert.match(summary, /Reprise de la conversation/);
  assert.match(summary, /caracteres d'historique/);
  assert.ok(
    !summary.includes("UTILISATEUR:"),
    "le corps de l'amorce ne fuit jamais dans la bulle",
  );
});

test("les deux chemins d'envoi passent la bulle par le meme filtre", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

  const directlyWired = main.match(/text: chatBubbleText\(prompt, submission\.displayText\)/g) ?? [];
  assert.equal(
    directlyWired.length,
    1,
    "le chat principal doit utiliser directement chatBubbleText",
  );
  assert.match(main, /const displayPrompt = chatBubbleText\(prompt, submission\.displayText\)/);
  const expertWired = main.match(/text: displayPrompt/g) ?? [];
  assert.equal(
    expertWired.length,
    2,
    "l'orchestration forcee et le chat expert doivent partager le texte filtre",
  );
  assert.ok(
    !/text: prompt,\n\s+timestamp: Math\.floor/.test(main),
    "aucune bulle ne doit plus afficher le prompt brut",
  );
  // Le transfert de compte est le seul emetteur d'amorce : il doit resumer.
  assert.match(main, /displayText: resumeSeedBubbleText\(prompt\)/);
});

test("le composeur borne un collage accidentel", () => {
  const view = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
  assert.match(view, /maxlength="\$\{CHAT_COMPOSER_MAX_CHARS\}"/);
  assert.ok(CHAT_COMPOSER_MAX_CHARS > CHAT_BUBBLE_MAX_CHARS);
});
