import assert from "node:assert/strict";
import test from "node:test";

import {
  TerminalInputBuffer,
  terminalInputDelivery,
  terminalReconnectPlan,
  terminalTransportErrorMessage,
} from "../src/terminal-transport.ts";

test("la saisie recue pendant la connexion est restituee dans l'ordre", () => {
  const buffer = new TerminalInputBuffer();
  buffer.append(12, "bonjour");
  buffer.append(12, " le terminal");

  assert.equal(buffer.take(12), "bonjour le terminal");
  assert.equal(buffer.take(12), "");
});

test("la saisie suit l'identifiant definitif attribue par le serveur", () => {
  const buffer = new TerminalInputBuffer();
  buffer.append(12, "avant");
  buffer.append(34, "apres");

  buffer.move(12, 34);

  assert.equal(buffer.take(12), "");
  assert.equal(buffer.take(34), "avantapres");
});

test("fermer un terminal supprime sa saisie en attente", () => {
  const buffer = new TerminalInputBuffer();
  buffer.append(12, "a ne pas envoyer");

  buffer.clear(12);

  assert.equal(buffer.take(12), "");
});

test("la taille de la file compte les caracteres pour le badge d'etat", () => {
  const buffer = new TerminalInputBuffer();
  assert.equal(buffer.size(7), 0);

  buffer.append(7, "abc");
  buffer.append(7, "dé");

  assert.equal(buffer.size(7), 5);
  assert.equal(buffer.take(7), "abcdé");
  assert.equal(buffer.size(7), 0);
});

test("une reconnexion WebSocket utilise REST sans bloquer la saisie", () => {
  assert.equal(terminalInputDelivery("open", false, false), "socket");
  assert.equal(terminalInputDelivery("connecting", false, false), "post");
  assert.equal(terminalInputDelivery("closed", false, false), "post");
  assert.equal(terminalInputDelivery("connecting", true, false), "buffer");
  assert.equal(terminalInputDelivery("connecting", false, true), "buffer");
});

test("la reconnexion d'un terminal vivant ne s'epuise jamais", () => {
  let plan = terminalReconnectPlan(0);
  assert.deepEqual(plan, { attempt: 1, delayMs: 250 });

  for (let index = 0; index < 200; index += 1) {
    plan = terminalReconnectPlan(plan.attempt);
  }

  assert.equal(plan.attempt, 32);
  assert.equal(plan.delayMs, 10_000);
  assert.deepEqual(terminalReconnectPlan(Number.POSITIVE_INFINITY), {
    attempt: 1,
    delayMs: 250,
  });
});

test("une panne fetch devient un message terminal utile", () => {
  const message = terminalTransportErrorMessage(
    "http://127.0.0.1:8080",
    new TypeError("Failed to fetch"),
  );

  assert.match(message, /Serveur terminal inaccessible/);
  assert.match(message, /127\.0\.0\.1:8080/);
  assert.doesNotMatch(message, /TypeError|Failed to fetch/);
});

test("une erreur applicative du serveur reste precise", () => {
  assert.equal(
    terminalTransportErrorMessage("http://127.0.0.1:8080", new Error("Session terminal introuvable")),
    "Error: Session terminal introuvable",
  );
});
