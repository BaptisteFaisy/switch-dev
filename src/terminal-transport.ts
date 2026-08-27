export class TerminalInputBuffer {
  private readonly pending = new Map<number, string[]>();

  append(id: number, data: string) {
    if (!data) return;
    const chunks = this.pending.get(id) ?? [];
    chunks.push(data);
    this.pending.set(id, chunks);
  }

  take(id: number) {
    const chunks = this.pending.get(id);
    this.pending.delete(id);
    return chunks?.join("") ?? "";
  }

  has(id: number) {
    return (this.pending.get(id)?.length ?? 0) > 0;
  }

  /** Nombre total de caracteres en attente pour ce terminal (badge d'etat). */
  size(id: number) {
    return (this.pending.get(id) ?? []).reduce((total, chunk) => total + chunk.length, 0);
  }

  move(from: number, to: number) {
    if (from === to) return;
    const source = this.pending.get(from);
    if (!source?.length) return;
    const destination = this.pending.get(to) ?? [];
    this.pending.delete(from);
    this.pending.set(to, [...source, ...destination]);
  }

  clear(id: number) {
    this.pending.delete(id);
  }
}

export type TerminalInputDelivery = "socket" | "buffer" | "post";

export type TerminalReconnectPlan = {
  attempt: number;
  delayMs: number;
};

/**
 * Les coupures du WebSocket ne prouvent pas que le PTY serveur est termine.
 * La reconnexion reste donc permanente, avec un compteur et un delai plafonnes.
 */
export const terminalReconnectPlan = (previousAttempt: number): TerminalReconnectPlan => {
  const safePrevious = Number.isFinite(previousAttempt)
    ? Math.max(0, Math.trunc(previousAttempt))
    : 0;
  const attempt = Math.min(safePrevious + 1, 32);
  return {
    attempt,
    delayMs: Math.min(10_000, 250 * 2 ** Math.min(attempt - 1, 6)),
  };
};

export const terminalInputDelivery = (
  socketState: "open" | "connecting" | "closed",
  terminalStarting: boolean,
  pendingInput: boolean,
): TerminalInputDelivery => {
  if (socketState === "open") return "socket";
  // Avant que le serveur ait attribue l'identifiant definitif, le POST ne peut
  // pas cibler le bon PTY. Une saisie deja tamponnee garde egalement son ordre.
  if (terminalStarting || pendingInput) return "buffer";
  // Pendant une simple reconnexion, REST reste disponible et evite de rendre
  // le terminal muet jusqu'a l'ouverture du nouveau WebSocket.
  return "post";
};

export const terminalTransportErrorMessage = (baseUrl: string, error: unknown) => {
  const raw = String(error);
  if (
    error instanceof TypeError ||
    /failed to fetch|networkerror|network request failed|load failed/i.test(raw)
  ) {
    return `Serveur terminal inaccessible (${baseUrl}). Reconnexion en cours...`;
  }
  return raw;
};
