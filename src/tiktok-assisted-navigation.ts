import "./tiktok-assisted-navigation.css";

export const TIKTOK_ASSISTED_NAVIGATION_SKILL_ID = "switch-tiktok-assisted-navigation";
export const TIKTOK_ASSISTED_NAVIGATION_SOURCE = "tiktok-assisted-navigation";

type AssistedDeviceAction =
  | "info"
  | "screenshot"
  | "open_screen"
  | "tap"
  | "swipe"
  | "type_text"
  | "key_event"
  | "open_app"
  | "shell";

type AssistedActionRequest = {
  deviceKey?: string;
  deviceId?: string;
  action: AssistedDeviceAction;
  args?: Record<string, unknown>;
  confirmed?: boolean;
  exactConfirmation?: string;
  source: string;
};

type RequestManualAction = (trigger: Event, request: AssistedActionRequest) => void;

export type TikTokAssistedNavigationInstaller = {
  requestManualAction: RequestManualAction;
  panelReadyEvent: string;
  slotSelector: string;
};

type SlotContext = {
  slot: HTMLElement;
  card: HTMLElement;
  deviceKey: string;
  deviceId: string;
  deviceName: string;
  android: boolean;
  ready: boolean;
};

type JournalEntry = {
  at: number;
  label: string;
  tone: "neutral" | "success" | "error";
};

const MAX_JOURNAL_ENTRIES = 5;
const expandedDevices = new Set<string>();
const journalByDevice = new Map<string, JournalEntry[]>();
const feedbackByDevice = new Map<string, JournalEntry>();
const pendingFocusByDevice = new Map<string, string>();
let installed = false;

const escapeHtml = (value: unknown): string =>
  String(value ?? "").replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[character] ?? character);

const contextForSlot = (slot: HTMLElement): SlotContext | null => {
  const card = slot.closest<HTMLElement>("[data-device-card]");
  const deviceKey = slot.dataset.deviceKey?.trim() ?? "";
  const deviceId = slot.dataset.deviceId?.trim() ?? "";
  if (!card || !deviceKey || !deviceId) return null;
  const deviceName = card.querySelector<HTMLElement>(".device-fleet-card-copy > strong")
    ?.textContent?.trim() || "Android sélectionné";
  return {
    slot,
    card,
    deviceKey,
    deviceId,
    deviceName,
    android: card.classList.contains("platform-android"),
    ready: card.classList.contains("state-ready"),
  };
};

const appendJournal = (
  deviceKey: string,
  label: string,
  tone: JournalEntry["tone"] = "neutral",
): JournalEntry => {
  const entry = { at: Date.now(), label, tone };
  const current = journalByDevice.get(deviceKey) ?? [];
  journalByDevice.set(deviceKey, [entry, ...current].slice(0, MAX_JOURNAL_ENTRIES));
  feedbackByDevice.set(deviceKey, entry);
  return entry;
};

const removeJournalEntry = (deviceKey: string, entry: JournalEntry): void => {
  const current = journalByDevice.get(deviceKey) ?? [];
  const next = current.filter((candidate) => candidate !== entry);
  if (next.length) journalByDevice.set(deviceKey, next);
  else journalByDevice.delete(deviceKey);
  if (feedbackByDevice.get(deviceKey) === entry) feedbackByDevice.delete(deviceKey);
};

const timeLabel = (timestamp: number): string =>
  new Intl.DateTimeFormat("fr-FR", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(timestamp);

const renderJournal = (deviceKey: string): string => {
  const journal = journalByDevice.get(deviceKey) ?? [];
  if (!journal.length) return "";
  return `<ol class="tiktok-assisted-journal" aria-label="Journal local de la session">
    ${journal.map((entry) => `<li class="is-${entry.tone}"><time datetime="${new Date(entry.at).toISOString()}">${timeLabel(entry.at)}</time><span>${escapeHtml(entry.label)}</span></li>`).join("")}
  </ol>`;
};

const availabilityMessage = (context: SlotContext): string => {
  if (!context.android) return "Disponible uniquement pour un appareil Android.";
  if (!context.ready) return "Autorise et rends cet Android disponible avant de commencer.";
  return "Prêt pour une navigation manuelle, une action explicite à la fois.";
};

const renderSlot = (context: SlotContext): void => {
  const available = context.android && context.ready;
  if (!available) expandedDevices.delete(context.deviceKey);
  const expanded = available && expandedDevices.has(context.deviceKey);
  const feedback = feedbackByDevice.get(context.deviceKey);
  context.slot.innerHTML = `<div class="tiktok-assisted-card ${expanded ? "is-expanded" : ""}">
    <button type="button" class="tiktok-assisted-toggle" data-tiktok-assisted-toggle aria-expanded="${expanded}" ${available ? "" : "disabled"}>
      <span class="tiktok-assisted-toggle-mark" aria-hidden="true">M</span>
      <span><strong>Utilisation humaine</strong><small>${escapeHtml(availabilityMessage(context))}</small></span>
      <b>${expanded ? "Fermer" : "Ouvrir"}</b>
    </button>
    ${expanded ? `<div class="tiktok-assisted-body">
      <div class="tiktok-assisted-heading">
        <div><small>Contrôle manuel</small><strong>${escapeHtml(context.deviceName)}</strong></div>
        <span>Utilisateur aux commandes</span>
      </div>
      <p>Ce mode ne lance aucune boucle. Chaque geste sur le téléphone doit venir d’un clic ou d’une confirmation distincte.</p>
      <ul class="tiktok-assisted-guardrails">
        <li><span>1</span>Un clic humain pour chaque action</li>
        <li><span>2</span>Aucun like, commentaire ou profil choisi automatiquement</li>
        <li><span>3</span>Arrêt sur CAPTCHA, restriction ou écran ambigu</li>
      </ul>
      <div class="tiktok-assisted-actions">
        <button type="button" data-tiktok-assisted-action="screenshot">Capturer l’écran</button>
        <button type="button" data-tiktok-assisted-action="open_screen">Ouvrir l’écran</button>
        <button type="button" data-tiktok-assisted-controls>Commandes manuelles</button>
        <button type="button" data-tiktok-assisted-skill>Ouvrir le skill</button>
      </div>
      <p class="tiktok-assisted-policy">Pas de hasard comportemental, de session autonome, de faux engagement ni d’imitation anti-détection.</p>
      ${feedback ? `<p class="tiktok-assisted-feedback is-${feedback.tone}" role="status">${escapeHtml(feedback.label)}</p>` : ""}
      ${renderJournal(context.deviceKey)}
    </div>` : ""}
  </div>`;
};

const rerenderSlot = (slot: HTMLElement): SlotContext | null => {
  const context = contextForSlot(slot);
  if (context) renderSlot(context);
  return context;
};

const focusControl = (context: SlotContext, selector: string): void => {
  context.slot.querySelector<HTMLElement>(selector)?.focus({ preventScroll: true });
};

const restorePendingFocus = (context: SlotContext): void => {
  const selector = pendingFocusByDevice.get(context.deviceKey);
  if (!selector) return;
  pendingFocusByDevice.delete(context.deviceKey);
  focusControl(context, selector);
};

const revealTikTokSkillCard = (): boolean => {
  const card = document.querySelector<HTMLElement>(
    `[data-skill-id="${TIKTOK_ASSISTED_NAVIGATION_SKILL_ID}"]`,
  );
  if (!card) return false;
  const details = card.querySelector<HTMLDetailsElement>("details.skill-details");
  if (details) details.open = true;
  card.scrollIntoView({ behavior: "smooth", block: "center" });
  card.focus({ preventScroll: true });
  return true;
};

const openManualControls = (
  context: SlotContext,
  requestManualAction: RequestManualAction,
): void => {
  const controls = context.card.querySelector<HTMLDetailsElement>("[data-device-controls]");
  if (!controls) {
    appendJournal(context.deviceKey, "Commandes manuelles indisponibles.", "error");
    renderSlot(context);
    bindSlot(context, requestManualAction);
    return;
  }
  controls.open = true;
  appendJournal(context.deviceKey, "Commandes manuelles affichées.", "success");
  renderSlot(context);
  bindSlot(context, requestManualAction);
  controls.scrollIntoView({ block: "nearest" });
  controls.querySelector<HTMLElement>("summary")?.focus({ preventScroll: true });
};

const openSkillLibrary = (
  context: SlotContext,
  requestManualAction: RequestManualAction,
): void => {
  const skillsButton = document.querySelector<HTMLButtonElement>(
    "#skillsToggle, [data-view='skills']",
  );
  if (!skillsButton) {
    appendJournal(context.deviceKey, "Bibliothèque de skills indisponible.", "error");
    renderSlot(context);
    bindSlot(context, requestManualAction);
    return;
  }
  appendJournal(context.deviceKey, "Ouverture du skill Navigation assistée.", "success");
  let observer: MutationObserver | null = null;
  let timeoutId: number | null = null;
  const reveal = (): boolean => {
    if (!revealTikTokSkillCard()) return false;
    observer?.disconnect();
    if (timeoutId !== null) window.clearTimeout(timeoutId);
    return true;
  };
  if (typeof MutationObserver !== "undefined") {
    observer = new MutationObserver(() => reveal());
    observer.observe(document.body, { childList: true, subtree: true });
  }
  skillsButton.click();
  if (!reveal()) {
    timeoutId = window.setTimeout(() => observer?.disconnect(), 3_000);
  }
};

const bindSlot = (
  context: SlotContext,
  requestManualAction: RequestManualAction,
): void => {
  context.slot.querySelector<HTMLButtonElement>("[data-tiktok-assisted-toggle]")
    ?.addEventListener("click", () => {
      if (expandedDevices.has(context.deviceKey)) expandedDevices.delete(context.deviceKey);
      else expandedDevices.add(context.deviceKey);
      renderSlot(context);
      bindSlot(context, requestManualAction);
      focusControl(context, "[data-tiktok-assisted-toggle]");
    });

  context.slot.querySelectorAll<HTMLButtonElement>("[data-tiktok-assisted-action]")
    .forEach((button) => {
      button.addEventListener("click", (event) => {
        const action = button.dataset.tiktokAssistedAction;
        if (action !== "screenshot" && action !== "open_screen") return;
        const currentContext = contextForSlot(context.slot);
        const focusSelector = `[data-tiktok-assisted-action="${action}"]`;
        if (
          !currentContext
          || !currentContext.slot.isConnected
          || !currentContext.android
          || !currentContext.ready
        ) {
          appendJournal(
            context.deviceKey,
            "L’Android n’est plus disponible. Aucune action envoyée.",
            "error",
          );
          if (currentContext?.slot.isConnected) {
            renderSlot(currentContext);
            bindSlot(currentContext, requestManualAction);
          }
          return;
        }
        const pendingEntry = appendJournal(
          currentContext.deviceKey,
          action === "screenshot"
            ? "Capture transmise au contrôleur."
            : "Ouverture de l’écran transmise au contrôleur.",
          "neutral",
        );
        pendingFocusByDevice.set(currentContext.deviceKey, focusSelector);
        try {
          requestManualAction(event, {
            deviceKey: currentContext.deviceKey,
            deviceId: currentContext.deviceId,
            action,
            ...(action === "open_screen" ? { confirmed: true } : {}),
            source: TIKTOK_ASSISTED_NAVIGATION_SOURCE,
          });
        } catch (error) {
          removeJournalEntry(currentContext.deviceKey, pendingEntry);
          appendJournal(
            currentContext.deviceKey,
            error instanceof Error ? error.message : String(error),
            "error",
          );
          pendingFocusByDevice.delete(currentContext.deviceKey);
          const liveContext = contextForSlot(currentContext.slot);
          if (liveContext?.slot.isConnected) {
            renderSlot(liveContext);
            bindSlot(liveContext, requestManualAction);
            focusControl(liveContext, focusSelector);
          }
        }
      });
    });

  context.slot.querySelector<HTMLButtonElement>("[data-tiktok-assisted-controls]")
    ?.addEventListener("click", () => openManualControls(context, requestManualAction));
  context.slot.querySelector<HTMLButtonElement>("[data-tiktok-assisted-skill]")
    ?.addEventListener("click", () => openSkillLibrary(context, requestManualAction));
};

const mountSlots = (
  selector: string,
  requestManualAction: RequestManualAction,
): void => {
  document.querySelectorAll<HTMLElement>(selector).forEach((slot) => {
    const context = rerenderSlot(slot);
    if (context) {
      bindSlot(context, requestManualAction);
      restorePendingFocus(context);
    }
  });
};

export const installTikTokAssistedNavigation = ({
  requestManualAction,
  panelReadyEvent,
  slotSelector,
}: TikTokAssistedNavigationInstaller): void => {
  if (installed || typeof window === "undefined") return;
  installed = true;
  const mount = () => mountSlots(slotSelector, requestManualAction);
  window.addEventListener(panelReadyEvent, mount);
  mount();
};
