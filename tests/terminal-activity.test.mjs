import assert from "node:assert/strict";
import test from "node:test";

import {
  TERMINAL_THINKING_IDLE_MS,
  createTerminalActivityTracker,
} from "../src/terminal-activity.ts";

const NOW = 1_000_000;

test("une sortie recente et volumineuse = travail en cours", () => {
  const tracker = createTerminalActivityTracker();
  tracker.record("a", "x".repeat(2_000), NOW);
  assert.equal(tracker.status("a", true, NOW + 100), "running");
});

test("une sortie qui se tait repasse au vert", () => {
  const tracker = createTerminalActivityTracker();
  tracker.record("a", "x".repeat(2_000), NOW);
  assert.equal(tracker.status("a", true, NOW + 100), "running");
  assert.equal(
    tracker.status("a", true, NOW + TERMINAL_THINKING_IDLE_MS + 50),
    "idle",
  );
});

test("un TUI au repos qui repeint sa barre de statut reste vert", () => {
  const tracker = createTerminalActivityTracker();
  // Repeints reguliers mais minuscules (spinner lent, barre de statut) : la
  // derniere sortie est recente, mais le volume de la fenetre reste sous le
  // seuil -> la pastille ne doit pas rester orange indefiniment.
  for (let i = 0; i < 20; i++) {
    tracker.record("a", "\r\u001b[Kstatus", NOW + i * 250);
  }
  assert.equal(tracker.status("a", true, NOW + 20 * 250), "idle");
});

test("un volume real sur la fenetre maintient le vert seulement apres le silence", () => {
  const tracker = createTerminalActivityTracker();
  // Une rafale de travail de 400 octets puis plus rien : orange pendant la
  // rafale, vert des que la sortie se tait.
  // Le seuil ACTIVITY_MIN_BYTES = 1024, il faut le depasser.
  tracker.record("a", "x".repeat(1_100), NOW);
  assert.equal(tracker.status("a", true, NOW + 50), "running");
  assert.equal(
    tracker.status("a", true, NOW + TERMINAL_THINKING_IDLE_MS + 50),
    "idle",
  );
});

test("un tour ferme est gris meme avec une sortie recente", () => {
  const tracker = createTerminalActivityTracker();
  tracker.record("a", "x".repeat(2_000), NOW);
  assert.equal(tracker.status("a", false, NOW + 100), "off");
});

test("une session rattachee sans sortie est verte jusqu'a la prochaine sortie", () => {
  const tracker = createTerminalActivityTracker();
  tracker.touch("a", NOW);
  assert.equal(tracker.status("a", true, NOW + 100), "idle");
  tracker.record("a", "x".repeat(2_000), NOW + 200);
  assert.equal(tracker.status("a", true, NOW + 300), "running");
});

test("forget oublie l'historique d'une session fermee", () => {
  const tracker = createTerminalActivityTracker();
  tracker.record("a", "x".repeat(2_000), NOW);
  tracker.forget("a");
  assert.equal(tracker.status("a", true, NOW + 100), "idle");
});
