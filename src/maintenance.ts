import { invoke } from "./platform";
import "./maintenance.css";

export type CleanupResultEntry = {
  label: string;
  freedBytes: number;
};

export type CleanupResult = {
  startedAt?: number;
  finishedAt?: number;
  freedTotalBytes?: number;
  entries?: CleanupResultEntry[];
  error?: string;
};

type CleanupStatus = {
  pending: boolean;
  requestedAt: number;
  lastResult: CleanupResult | null;
};

let status: CleanupStatus | null = null;
let requesting = false;
let errorMessage = "";
let active = false;
let pollTimer: number | null = null;

const escapeHtml = (value: unknown): string =>
  String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");

const formatBytes = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 o";
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} Ko`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} Mo`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} Go`;
};

const formatWhen = (unixSeconds: number | undefined): string => {
  if (!unixSeconds) return "";
  return new Intl.DateTimeFormat("fr-FR", {
    dateStyle: "short",
    timeStyle: "medium",
  }).format(new Date(unixSeconds * 1_000));
};

const resultSummary = (result: CleanupResult): string => {
  const parts: string[] = [];
  for (const entry of result.entries ?? []) {
    parts.push(`${escapeHtml(entry.label)} : ${formatBytes(entry.freedBytes)}`);
  }
  if (result.error) parts.push(`Attention : ${escapeHtml(result.error)}`);
  return parts.join(" · ");
};

const stopPolling = (): void => {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
};

export const refreshMaintenanceCard = async (
  rerender: () => void,
  silent = false,
): Promise<void> => {
  try {
    status = await invoke<CleanupStatus>("cleanup_status");
    errorMessage = "";
  } catch (error) {
    if (!silent) errorMessage = String(error);
  } finally {
    if (active && !silent) rerender();
  }
};

const schedulePolling = (rerender: () => void): void => {
  stopPolling();
  pollTimer = window.setInterval(() => {
    void (async () => {
      await refreshMaintenanceCard(rerender, true);
      if (status && !status.pending) {
        stopPolling();
        rerender();
      }
    })();
  }, 4_000);
};

export const renderMaintenanceCard = (): string => {
  const pending = status?.pending ?? false;
  const last = status?.lastResult ?? null;
  const lastLine = last ? resultSummary(last) : "";
  const lastWhen = formatWhen(last?.finishedAt);
  return `
    <section class="appearance-settings maintenance-card">
      <h2 class="settings-section-title">Maintenance du PC</h2>
      <p class="settings-hint">
        Nettoie les fichiers inutiles de ce PC (temporaires Windows, caches npm,
        caches Docker et fichiers temporaires du conteneur Switch). Le nettoyage
        est lance par l'agent Windows local ; il ne touche a aucun projet ni
        aucune conversation.
      </p>
      ${
        pending
          ? '<p class="maintenance-state maintenance-state-pending" aria-live="polite">Nettoyage en cours sur le PC... Cette page se met a jour toute seule.</p>'
          : ""
      }
      ${errorMessage ? `<div class="maintenance-alert" role="alert">${escapeHtml(errorMessage)}</div>` : ""}
      ${
        last
          ? `<p class="maintenance-last-result">${
              last.error ? "" : "Dernier nettoyage"
            }${lastWhen ? ` (${escapeHtml(lastWhen)})` : ""} : ${
              last.freedTotalBytes
                ? `<strong>${formatBytes(last.freedTotalBytes)} libérés</strong>`
                : escapeHtml(lastLine || "terminé")
            }${last.freedTotalBytes && lastLine ? `<br /><span class="maintenance-detail">${lastLine}</span>` : ""}</p>`
          : ""
      }
      <button
        id="maintenanceCleanButton"
        class="chat-button maintenance-clean-button"
        type="button"
        ${pending || requesting ? "disabled" : ""}
      >
        ${pending || requesting ? "Nettoyage en cours..." : "Nettoyer mon PC maintenant"}
      </button>
    </section>
  `;
};

export const bindMaintenanceCard = (rerender: () => void): void => {
  document
    .querySelector("#maintenanceCleanButton")
    ?.addEventListener("click", () => {
      if (requesting || status?.pending) return;
      requesting = true;
      errorMessage = "";
      rerender();
      void (async () => {
        try {
          await invoke<CleanupStatus>("cleanup_request");
          await refreshMaintenanceCard(rerender, true);
          schedulePolling(rerender);
        } catch (error) {
          errorMessage = String(error);
        } finally {
          requesting = false;
          rerender();
        }
      })();
    });
};

export const activateMaintenanceCard = (rerender: () => void): void => {
  active = true;
  void refreshMaintenanceCard(rerender, true).then(() => {
    if (status?.pending) schedulePolling(rerender);
  });
};

export const deactivateMaintenanceCard = (): void => {
  active = false;
  stopPolling();
};
