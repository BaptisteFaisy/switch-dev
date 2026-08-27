/**
 * Heuristique d'activité d'un terminal.
 *
 * La pastille d'un terminal est verte au repos, orange pendant le travail, et
 * grise quand la session est fermée. La seule information disponible côté
 * client est le flux de sortie du PTY : les CLI d'agent animent un indicateur
 * tant qu'ils travaillent, puis se taisent en rendant la main.
 *
 * Sans garde-fou, un TUI au repos qui repeint périodiquement sa barre de statut
 * (spinner, compteur, redessin sur redimensionnement) maintiendrait la pastille
 * orange indéfiniment, même une fois le tour terminé. Deux signaux sont donc
 * combinés :
 *
 *  - la fraîcheur : la dernière sortie doit dater de moins de
 *    `TERMINAL_THINKING_IDLE_MS`. Une sortie qui se tait repasse au vert.
 *  - le volume : la sortie des dernières `ACTIVITY_WINDOW_MS` doit dépasser
 *    `ACTIVITY_MIN_BYTES`. Un repeint faible et régulier (statut, spinner
 *    lent) est traité comme du repos, pas comme du travail.
 *
 * C'est une heuristique et non une mesure : un agent qui réfléchit sans rien
 * écrire passera au vert, et un repeint plein écran rapide peut rester orange.
 */
export const TERMINAL_THINKING_IDLE_MS = 2_000;
const ACTIVITY_WINDOW_MS = 8_000;
const ACTIVITY_MIN_BYTES = 1_024;

export type TerminalActivityStatus = "off" | "running" | "idle";

type OutputSample = { at: number; bytes: number };

export type TerminalActivityTracker = ReturnType<typeof createTerminalActivityTracker>;

export const createTerminalActivityTracker = () => {
  const lastOutputAt = new Map<string, number>();
  const windows = new Map<string, OutputSample[]>();

  /** Enregistre une sortie réelle du terminal (octets du PTY). */
  const record = (key: string, data: string, at = Date.now()): void => {
    lastOutputAt.set(key, at);
    const samples = windows.get(key) ?? [];
    samples.push({ at, bytes: data.length });
    windows.set(key, samples);
    while (samples.length > 0 && at - samples[0].at > ACTIVITY_WINDOW_MS) {
      samples.shift();
    }
  };

  /** Marque la sortie comme récente sans volume (rattachement d'une session). */
  const touch = (key: string, at = Date.now()): void => {
    lastOutputAt.set(key, at);
  };

  /** Oublie l'historique d'une session fermée. */
  const forget = (key: string): void => {
    lastOutputAt.delete(key);
    windows.delete(key);
  };

  const windowBytes = (key: string): number => {
    const samples = windows.get(key);
    if (!samples || samples.length === 0) return 0;
    let total = 0;
    for (const sample of samples) total += sample.bytes;
    return total;
  };

  const status = (
    key: string,
    running: boolean,
    at = Date.now(),
  ): TerminalActivityStatus => {
    if (!running) return "off";
    const last = lastOutputAt.get(key) ?? 0;
    if (at - last >= TERMINAL_THINKING_IDLE_MS) return "idle";
    if (windowBytes(key) < ACTIVITY_MIN_BYTES) return "idle";
    return "running";
  };

  return { record, touch, forget, status };
};
