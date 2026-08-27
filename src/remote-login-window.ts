export const CODEX_DEVICE_VERIFICATION_URL = "https://auth.openai.com/codex/device";
const LOGIN_OUTPUT_LIMIT = 16_384;

type RemoteLoginPhase = "preparing" | "ready" | "success" | "error";

type RemoteLoginWindowState = {
  accountId: string;
  accountLabel: string;
  popup: Window | null;
  output: string;
  phase: RemoteLoginPhase;
  userCode: string | null;
  verificationUrl: string | null;
};

export type RemoteLoginWindowUpdate =
  | { type: "none" }
  | { type: "ready"; userCode: string; verificationUrl: string }
  | { type: "success" }
  | { type: "error"; message: string };

const remoteLoginWindows = new Map<string, RemoteLoginWindowState>();

const popupName = (accountId: string) =>
  `cst-codex-login-${accountId.replace(/[^a-z0-9_-]/gi, "-")}`;

const popupIsUsable = (popup: Window | null | undefined): popup is Window => {
  if (!popup) return false;
  try {
    return !popup.closed;
  } catch {
    return false;
  }
};

const popupDocument = (state: RemoteLoginWindowState): Document | null => {
  if (!popupIsUsable(state.popup)) return null;
  try {
    return state.popup.document;
  } catch {
    // La fenetre a deja navigue vers auth.openai.com (origine differente).
    return null;
  }
};

const writePopupShell = (state: RemoteLoginWindowState) => {
  const document = popupDocument(state);
  if (!document) return;
  document.open();
  document.write(`<!doctype html>
<html lang="fr">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <title>Connexion Codex</title>
    <style>
      :root { color-scheme: dark; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
      * { box-sizing: border-box; }
      body { margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 24px; color: #f5f7fb; background: radial-gradient(circle at top, #24375d 0, #111827 48%, #080d18 100%); }
      main { width: min(100%, 460px); padding: 32px; border: 1px solid rgba(255,255,255,.14); border-radius: 22px; background: rgba(15,23,42,.9); box-shadow: 0 24px 70px rgba(0,0,0,.42); }
      .mark { width: 52px; height: 52px; display: grid; place-items: center; margin-bottom: 20px; border-radius: 16px; background: #fff; color: #111827; font-size: 25px; font-weight: 800; }
      h1 { margin: 0 0 8px; font-size: 26px; letter-spacing: -.02em; }
      p { margin: 0; color: #b8c2d8; line-height: 1.55; }
      #account { margin-top: 6px; color: #fff; font-weight: 650; }
      .loader { width: 34px; height: 34px; margin: 28px auto 14px; border: 3px solid rgba(255,255,255,.16); border-top-color: #7dd3fc; border-radius: 999px; animation: spin .8s linear infinite; }
      @keyframes spin { to { transform: rotate(360deg); } }
      #codeWrap { display: none; margin: 26px 0 18px; text-align: center; }
      #code { display: block; padding: 17px 12px; border: 1px solid rgba(125,211,252,.42); border-radius: 14px; color: #e0f2fe; background: rgba(14,116,144,.16); font: 750 28px/1.1 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; letter-spacing: .08em; user-select: all; }
      button { width: 100%; display: none; margin-top: 18px; padding: 14px 18px; border: 0; border-radius: 12px; color: #07111f; background: #e0f2fe; font: inherit; font-weight: 750; cursor: pointer; }
      button:hover { background: #bae6fd; }
      button:focus-visible { outline: 3px solid #38bdf8; outline-offset: 3px; }
      #hint { margin-top: 13px; font-size: 13px; text-align: center; }
      .success .loader, .error .loader { display: none; }
      .success .mark { color: #052e16; background: #bbf7d0; }
      .error .mark { color: #450a0a; background: #fecaca; }
    </style>
  </head>
  <body>
    <main id="card" aria-live="polite">
      <div class="mark" id="mark">C</div>
      <h1 id="title">Connexion à Codex</h1>
      <p id="status">Préparation de la fenêtre sécurisée OpenAI…</p>
      <p id="account"></p>
      <div class="loader" id="loader" aria-label="Chargement"></div>
      <div id="codeWrap">
        <p>Code de connexion</p>
        <output id="code"></output>
      </div>
      <button type="button" id="continue">Copier le code et continuer avec OpenAI</button>
      <p id="hint"></p>
    </main>
  </body>
</html>`);
  document.close();
  const account = document.querySelector<HTMLElement>("#account");
  if (account) account.textContent = state.accountLabel;
};

const copyDeviceCode = (userCode: string) => {
  try {
    void window.navigator.clipboard?.writeText(userCode);
  } catch {
    // Le repli synchrone ci-dessous couvre les navigateurs sans Clipboard API.
  }
  try {
    const input = document.createElement("textarea");
    input.value = userCode;
    input.setAttribute("readonly", "");
    input.style.position = "fixed";
    input.style.opacity = "0";
    document.body.appendChild(input);
    input.select();
    document.execCommand("copy");
    input.remove();
  } catch {
    // Le code reste affiche et selectionnable si le presse-papiers est refuse.
  }
};

const renderReady = (state: RemoteLoginWindowState) => {
  if (!state.userCode) return;
  copyDeviceCode(state.userCode);
  state.popup?.focus();
};

const renderTerminalState = (
  state: RemoteLoginWindowState,
  phase: "success" | "error",
  message: string,
) => {
  const document = popupDocument(state);
  if (!document) return;
  const card = document.querySelector<HTMLElement>("#card");
  const mark = document.querySelector<HTMLElement>("#mark");
  const title = document.querySelector<HTMLElement>("#title");
  const status = document.querySelector<HTMLElement>("#status");
  const codeWrap = document.querySelector<HTMLElement>("#codeWrap");
  const button = document.querySelector<HTMLButtonElement>("#continue");
  const hint = document.querySelector<HTMLElement>("#hint");
  card?.classList.add(phase);
  if (mark) mark.textContent = phase === "success" ? "✓" : "!";
  if (title) title.textContent = phase === "success" ? "Connexion réussie" : "Connexion interrompue";
  if (status) status.textContent = message;
  if (codeWrap) codeWrap.style.display = "none";
  if (button) button.style.display = "none";
  if (hint) hint.textContent = phase === "success" ? "Cette fenêtre va se fermer automatiquement." : "Fermez cette fenêtre puis réessayez.";
};

export const stripRemoteLoginControlSequences = (value: string): string =>
  value
    .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
    .replaceAll("\r", "");

export const parseRemoteCodexLoginOutput = (value: string) => {
  const output = stripRemoteLoginControlSequences(value);
  const url = output.match(/https:\/\/auth\.openai\.com\/codex\/device\b/i)?.[0] ?? null;
  const userCode = output.match(/\b[A-Z0-9]{4}-[A-Z0-9]{4,5}\b/)?.[0] ?? null;
  const success = /(?:Successfully logged in|Login successful|Authentication successful)/i.test(output);
  const failed = /(?:device code (?:has )?expired|login (?:failed|timed out)|authentication (?:failed|timed out))/i.test(output);
  return { output, url, userCode, success, failed };
};

export const openRemoteCodexLoginWindow = (
  accountId: string,
  accountLabel: string,
): boolean => {
  const previous = remoteLoginWindows.get(accountId);
  if (previous && popupIsUsable(previous.popup) && previous.phase !== "error") {
    previous.popup.focus();
    return true;
  }
  if (previous) {
    try {
      previous.popup?.close();
    } catch {
      // Rien a nettoyer si le navigateur a deja detruit la fenetre.
    }
    remoteLoginWindows.delete(accountId);
  }

  // Sans parametre de dimensions, Chrome ouvre un onglet normal. L'URL OpenAI
  // est fournie des le clic afin que l'utilisateur n'attende pas sur une page
  // intermediaire pendant le demarrage du terminal de device-auth.
  const popup = window.open(CODEX_DEVICE_VERIFICATION_URL, "_blank");
  if (!popupIsUsable(popup)) return false;

  const state: RemoteLoginWindowState = {
    accountId,
    accountLabel,
    popup,
    output: "",
    phase: "preparing",
    userCode: null,
    verificationUrl: null,
  };
  remoteLoginWindows.set(accountId, state);
  popup.focus();
  return true;
};

// Les liens de la page Comptes ouvrent OpenAI nativement avec target="_blank".
// Ils n'exposent pas de WindowProxy, mais le suivi du device-auth doit quand
// meme rester actif pour copier le code et fermer le terminal apres validation.
export const prepareRemoteCodexLoginTab = (
  accountId: string,
  accountLabel: string,
): boolean => {
  const previous = remoteLoginWindows.get(accountId);
  if (previous?.phase === "preparing" || previous?.phase === "ready") {
    if (previous.userCode) copyDeviceCode(previous.userCode);
    return true;
  }
  remoteLoginWindows.set(accountId, {
    accountId,
    accountLabel,
    popup: null,
    output: "",
    phase: "preparing",
    userCode: null,
    verificationUrl: CODEX_DEVICE_VERIFICATION_URL,
  });
  return true;
};

export const remoteCodexLoginWindowIsOpen = (accountId: string): boolean => {
  const state = remoteLoginWindows.get(accountId);
  if (!state) return false;
  if (state.popup && !popupIsUsable(state.popup)) {
    remoteLoginWindows.delete(accountId);
    return false;
  }
  return state.phase === "preparing" || state.phase === "ready";
};

export const remoteCodexLoginWindowNeedsCode = (accountId: string): boolean => {
  const state = remoteLoginWindows.get(accountId);
  if (!state || (state.popup && !popupIsUsable(state.popup))) return false;
  return state.phase === "preparing";
};

export type RemoteCodexLoginPanel = {
  phase: "preparing" | "ready";
  userCode: string | null;
  verificationUrl: string;
};

// La page Comptes affiche un encadre permanent avec le code appareil tant que la
// connexion Codex est en cours. Contrairement au toast de statut, il ne disparait
// pas : l'utilisateur doit pouvoir relire puis recopier ce code a tout moment
// pour le saisir sur la page OpenAI.
export const remoteCodexLoginPanel = (
  accountId: string,
): RemoteCodexLoginPanel | null => {
  const state = remoteLoginWindows.get(accountId);
  if (!state || (state.popup && !popupIsUsable(state.popup))) return null;
  if (state.phase !== "preparing" && state.phase !== "ready") return null;
  return {
    phase: state.phase,
    userCode: state.userCode,
    verificationUrl: state.verificationUrl ?? CODEX_DEVICE_VERIFICATION_URL,
  };
};

// Recopie le code courant a la demande (bouton « Copier »). L'auto-copie initiale
// peut avoir ete perdue si l'utilisateur a copie autre chose entre-temps.
export const copyRemoteCodexLoginCode = (accountId: string): string | null => {
  const state = remoteLoginWindows.get(accountId);
  if (!state?.userCode) return null;
  copyDeviceCode(state.userCode);
  return state.userCode;
};

export const focusRemoteCodexLoginWindow = (accountId: string): boolean => {
  const state = remoteLoginWindows.get(accountId);
  if (!state || !popupIsUsable(state.popup)) return false;
  state.popup.focus();
  return true;
};

export const consumeRemoteCodexLoginOutput = (
  accountId: string,
  chunk: string,
): RemoteLoginWindowUpdate => {
  const state = remoteLoginWindows.get(accountId);
  if (!state || (state.popup && !popupIsUsable(state.popup))) return { type: "none" };
  state.output = `${state.output}${chunk}`.slice(-LOGIN_OUTPUT_LIMIT);
  const parsed = parseRemoteCodexLoginOutput(state.output);

  if (parsed.success && state.phase !== "success") {
    state.phase = "success";
    renderTerminalState(state, "success", "Votre compte est maintenant enregistré sur le VPS.");
    window.setTimeout(() => {
      try {
        state.popup?.close();
      } catch {
        // La page OpenAI peut deja avoir ferme sa fenetre.
      }
      if (remoteLoginWindows.get(accountId) === state) remoteLoginWindows.delete(accountId);
    }, 1_500);
    return { type: "success" };
  }

  if (parsed.failed && state.phase !== "error") {
    state.phase = "error";
    const message = "Le code a expiré ou OpenAI a refusé la connexion.";
    renderTerminalState(state, "error", message);
    return { type: "error", message };
  }

  if (parsed.url && parsed.userCode && state.phase === "preparing") {
    state.phase = "ready";
    state.verificationUrl = CODEX_DEVICE_VERIFICATION_URL;
    state.userCode = parsed.userCode;
    renderReady(state);
    return {
      type: "ready",
      userCode: state.userCode,
      verificationUrl: state.verificationUrl,
    };
  }

  return { type: "none" };
};

export const failRemoteCodexLoginWindow = (accountId: string, message: string) => {
  const state = remoteLoginWindows.get(accountId);
  if (!state || state.phase === "success") return;
  state.phase = "error";
  renderTerminalState(state, "error", message);
};
