export type RemoteWorkloadKind = "chat" | "terminal";

export type RemoteAllocationHealth = {
  ok: boolean;
  ready?: boolean;
  draining?: boolean;
  activeTerminals?: number;
  activeChatTurns?: number;
  capacity?: number;
  availableAccountIds?: string[];
};

export type RemoteAllocationObservation<T> = {
  node: T;
  health: RemoteAllocationHealth | null;
};

export type RankedRemoteAllocation<T> = RemoteAllocationObservation<T> & {
  score: number;
  saturated: boolean;
  healthKnown: boolean;
};

type PrioritizedNode = {
  priority: number;
};

const finiteNonNegative = (value: unknown): number => {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0;
};

const workloadCount = (
  health: RemoteAllocationHealth,
  _workload: RemoteWorkloadKind,
): number => {
  const chats = finiteNonNegative(health.activeChatTurns);
  return finiteNonNegative(health.activeTerminals) + chats;
};

/**
 * Classe les noeuds pour une nouvelle charge de travail.
 *
 * - un noeud explicitement indisponible ou en drain est exclu ;
 * - les noeuds sains non satures passent avant les noeuds satures ;
 * - une sonde inconnue reste un dernier recours pour tolerer un incident
 *   transitoire de la route de sante ;
 * - lorsqu'un serveur annonce ses comptes, un chat n'y est envoye que si le
 *   compte demande y existe. Les anciens serveurs qui n'annoncent pas ce champ
 *   restent compatibles.
 */
export const rankRemoteAllocations = <T extends PrioritizedNode>(
  observations: RemoteAllocationObservation<T>[],
  workload: RemoteWorkloadKind,
  accountId?: string | null,
): RankedRemoteAllocation<T>[] => {
  const ranked = observations.flatMap((observation) => {
    const { node, health } = observation;
    if (health === null) {
      return [{
        ...observation,
        score: Number.POSITIVE_INFINITY,
        saturated: true,
        healthKnown: false,
        tier: 2,
      }];
    }

    // `ready`/`draining` sont des signaux de maintenance ajoutés après le
    // premier protocole de santé. Un nœud ancien ou une réponse de proxy peut
    // les omettre : dans ce cas `ok: true` reste la preuve de disponibilité.
    if (health.ok === false || health.ready === false || health.draining === true) {
      return [];
    }
    if (
      accountId &&
      Array.isArray(health.availableAccountIds) &&
      !health.availableAccountIds.includes(accountId)
    ) {
      return [];
    }

    // `capacity: 0` signifie « sans plafond numerique » : la memoire du noeud
    // reste son garde-fou et il ne doit jamais etre classe comme sature par un
    // simple nombre de chats ou de terminaux.
    const capacity = finiteNonNegative(health.capacity);
    const active = workloadCount(health, workload);
    const saturated = capacity > 0 && active >= capacity;
    return [{
      ...observation,
      score: (capacity > 0 ? active / capacity : active) + finiteNonNegative(node.priority) / 100,
      saturated,
      healthKnown: true,
      tier: saturated ? 1 : 0,
    }];
  });

  return ranked
    .sort((left, right) =>
      left.tier - right.tier ||
      left.score - right.score ||
      left.node.priority - right.node.priority,
    )
    .map(({ tier: _tier, ...entry }) => entry);
};
