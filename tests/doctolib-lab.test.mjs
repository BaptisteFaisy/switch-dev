import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  appendDoctolibLabMessage,
  createDoctolibLabState,
  formatDoctolibLabSlot,
  interpretDoctolibLabMessage,
  renderDoctolibLabPanel,
  selectedDoctolibLabProposal,
} from "../src/doctolib-lab.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/doctolib_lab.rs", import.meta.url), "utf8");
const lib = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");
const worker = readFileSync(new URL("../scripts/doctolib-lab-worker.mjs", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const theme = readFileSync(new URL("../src/theme.css", import.meta.url), "utf8");
const view = readFileSync(new URL("../src/doctolib-lab-view.ts", import.meta.url), "utf8");
const viewStyle = readFileSync(new URL("../src/doctolib-lab.css", import.meta.url), "utf8");

const proposal = {
  id: "proposal-1",
  mode: "demo",
  practitionerName: "Dr Camille Martin (démo)",
  specialty: "Médecin généraliste",
  address: "12 rue du Laboratoire, Paris",
  sector: "Secteur 1",
  visitMotive: "Première consultation",
  startsAt: "2026-07-20T10:30:00+02:00",
  sourceUrl: "https://www.doctolib.fr/medecin-generaliste/paris",
  acceptsNewPatients: true,
  expiresAt: 1_800_000_000,
};

test("RDV Lab démarre sur le vrai compte et conserve un bac à sable explicite", () => {
  const state = createDoctolibLabState();
  assert.equal(state.mode, "live");
  assert.equal(state.location, "Paris");
  assert.equal(state.syncGoogleCalendar, true);

  state.mode = "demo";
  state.search = {
    mode: "demo",
    generatedAt: 1,
    recommendedProposalId: proposal.id,
    proposals: [proposal],
    note: "Données fictives",
  };
  state.selectedProposalId = proposal.id;

  assert.equal(selectedDoctolibLabProposal(state)?.id, proposal.id);
  const html = renderDoctolibLabPanel(state, { remoteMode: false });
  assert.match(html, /Feature séparée · expérimental/);
  assert.match(html, /Le bac à sable est actif/);
  assert.match(html, /Oui, prendre ce RDV/);
  assert.match(html, /Dr Camille Martin \(démo\)/);
  assert.match(html, /id="doctolibLabChatForm"/);
  assert.match(html, /id="doctolibLabGoogleCalendarSync"/);
  assert.match(html, /Désactivé dans le bac à sable/);
  assert.doesNotMatch(html, /id="doctolibLabSearchForm"/);
  assert.match(formatDoctolibLabSlot(proposal.startsAt), /lundi 20 juillet 2026 à 10:30/i);
});

test("le chat comprend une demande libre, les choix et la validation", () => {
  assert.deepEqual(
    interpretDoctolibLabMessage("Prends-moi un RDV avec un généraliste sur Paris demain"),
    { kind: "search", specialty: "Médecin généraliste", location: "Paris" },
  );
  assert.deepEqual(interpretDoctolibLabMessage("oui"), { kind: "confirm" });
  assert.deepEqual(
    interpretDoctolibLabMessage("Connecte Google Calendar"),
    { kind: "connect-calendar" },
  );
  assert.deepEqual(interpretDoctolibLabMessage("le deuxième créneau"), { kind: "select", index: 1 });
  assert.deepEqual(
    interpretDoctolibLabMessage("Lyon", { awaitingLocation: true }),
    { kind: "search", specialty: "Médecin généraliste", location: "Lyon" },
  );
  assert.deepEqual(
    interpretDoctolibLabMessage("Je cherche un dermatologue à Paris"),
    { kind: "unsupported-specialty", specialty: "dermatologue" },
  );
});

test("la conversation rend les messages utilisateur sans exécuter de HTML", () => {
  const state = createDoctolibLabState();
  appendDoctolibLabMessage(state, "user", '<img src=x onerror="alert(1)">');
  const html = renderDoctolibLabPanel(state, { remoteMode: false });
  assert.match(html, /&lt;img src=x onerror=&quot;alert\(1\)&quot;&gt;/);
  assert.doesNotMatch(html, /<img src=x/);
  assert.match(html, /data-doctolib-prompt="Trouve-moi un médecin généraliste à Paris"/);
});

test("le site :8080 utilise le moteur de la machine serveur", () => {
  const state = createDoctolibLabState();
  state.status = {
    demoReady: true,
    liveReady: true,
    nodeReady: true,
    workerReady: true,
    chromeReady: true,
    connected: true,
    googleCalendarReady: true,
    googleCalendarConnected: false,
    detail: "Moteur prêt",
  };
  const html = renderDoctolibLabPanel(state, { remoteMode: true });
  assert.match(html, /Compte Doctolib réel connecté sur le serveur/);
  assert.match(html, /machine du serveur :8080/);
  assert.match(html, /Le mode réel est actif/);
  assert.match(html, /id="doctolibLabGoogleCalendarConnect"/);
  assert.match(html, /Connecter Google Calendar/);
  assert.doesNotMatch(html, /type="submit" class="tool-button primary" disabled/);
});

test("la vue est séparée des chats et raccordée aux cinq commandes desktop", () => {
  assert.match(types, /\| "doctolib-lab"/);
  assert.doesNotMatch(main, /doctolibLabToggle/);
  assert.doesNotMatch(main, /data-view="doctolib-lab"/);
  assert.match(main, /type DoctolibLabModule = typeof import\("\.\/doctolib-lab-view"\)/);
  assert.match(main, /doctolibLabModulePromise = import\("\.\/doctolib-lab-view"\)/);
  assert.match(main, /if \(view === "doctolib-lab" && !doctolibLabModule\)/);
  assert.match(main, /import type \{[\s\S]*?\} from "\.\/doctolib-lab";/);
  assert.doesNotMatch(main, /import\s+\{[^}]*\}\s+from "\.\/doctolib-lab";/);
  assert.match(main, /renderDoctolibLabPanel/);
  assert.match(main, /handleDoctolibLabMessage/);
  assert.doesNotMatch(`${style}\n${theme}`, /\.doctolib-lab(?:[\s.:>]|$)/);
  assert.match(view, /import "\.\/doctolib-lab\.css";/);
  assert.match(view, /export \* from "\.\/doctolib-lab";/);
  assert.match(viewStyle, /\.doctolib-lab\s*\{/);
  assert.match(viewStyle, /:root\[data-theme="light"\] \.doctolib-lab/);
  for (const command of [
    "doctolib_lab_status",
    "doctolib_lab_connect",
    "doctolib_lab_google_calendar_connect",
    "doctolib_lab_search",
    "doctolib_lab_confirm",
  ]) {
    assert.match(main, new RegExp(command));
    assert.match(lib, new RegExp(command));
    assert.match(platform, new RegExp(command));
  }
  assert.match(platform, /\/api\/doctolib-lab\/status/);
  assert.match(platform, /\/api\/doctolib-lab\/search/);
  assert.match(platform, /\/api\/doctolib-lab\/google-calendar\/connect/);
  assert.match(server, /"\/doctolib-lab\/status"/);
  assert.match(server, /"\/doctolib-lab\/confirm"/);
  assert.match(server, /"\/doctolib-lab\/google-calendar\/connect"/);
  assert.match(server, /check_admin_header\(&state, &headers\)/);
});

test("le backend protège le clic Oui et le worker ne contourne pas l'authentification", () => {
  assert.match(backend, /PROPOSAL_TTL_SECONDS/);
  assert.match(backend, /pending\.remove\(proposal_id\)/);
  assert.match(backend, /booking_in_progress/);
  assert.match(backend, /validate_proposal/);
  assert.match(backend, /https:\/\/www\.doctolib\.fr\//);
  assert.match(worker, /launchPersistentContext/);
  assert.match(worker, /sessionStatus/);
  assert.match(worker, /request\.action === "session"/);
  assert.match(worker, /headless: false/);
  assert.match(worker, /loginRequired/);
  assert.match(worker, /data-test-id="slot-button"/);
  assert.match(worker, /exactAppointmentVisible/);
  assert.match(worker, /confirmedText \|\| exactAppointmentVisible/);
  assert.match(backend, /confirmation\.verified && add_to_google_calendar/);
  assert.match(backend, /google-calendar-profile/);
  assert.match(worker, /request\.action === "calendar_session"/);
  assert.match(worker, /request\.action === "calendar_connect"/);
  assert.match(worker, /request\.action === "calendar_add"/);
  assert.match(worker, /calendarTemplateUrl/);
  assert.match(worker, /savedNotice/);
  assert.match(worker, /calendar\.google\.com/);
  assert.doesNotMatch(worker, /captcha.*(?:bypass|solve)|(?:bypass|solve).*captcha/i);
  assert.doesNotMatch(worker, /(?:password|mot de passe)\s*[:=]\s*request/i);
});
