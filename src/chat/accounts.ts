export type ChatAccountReference = {
  id: string;
  label: string;
  provider?: "codex" | "claude" | "opencode" | "freebuff" | "aihubmix";
  codexHome: string;
};

/**
 * Les limites et les chats sont charges par deux endpoints differents.
 * Detecte qu'un ajout/suppression de compte (eventuellement fait depuis une
 * autre fenetre) impose de recharger les settings avant d'ouvrir un chat.
 */
export const accountCatalogMatchesLimitRows = (
  accounts: ChatAccountReference[],
  limitRows: ChatAccountReference[],
): boolean => {
  if (accounts.length !== limitRows.length) return false;
  const accountsById = new Map(accounts.map((account) => [account.id, account]));
  return limitRows.every((row) => {
    const account = accountsById.get(row.id);
    return (
      account?.label === row.label &&
      account.codexHome === row.codexHome &&
      (account.provider ?? "codex") === (row.provider ?? "codex")
    );
  });
};

const uniqueAccountsByHome = <T extends ChatAccountReference>(accounts: readonly T[]): T[] => {
  const unique: T[] = [];
  const seenHomes = new Set<string>();
  for (const account of accounts) {
    const rawHome = account.codexHome.trim();
    let home = rawHome.replace(/\\/g, "/").replace(/\/+$/, "");
    if (rawHome.includes("\\") || /^[a-z]:\//i.test(home) || /^%[^%]+%\//.test(home)) {
      home = home.toLocaleLowerCase("en-US");
    }
    const key = home || `profile:${account.id}`;
    if (seenHomes.has(key)) continue;
    seenHomes.add(key);
    unique.push(account);
  }
  return unique;
};

/**
 * La liste configuree reste la source de verite de la vue Limites, mais un
 * meme home d'authentification n'occupe jamais plusieurs lignes. Une ligne de
 * repli garde aussi un compte visible si la reponse de quotas est incomplete.
 */
export const accountLimitRowsForDisplay = <
  TAccount extends ChatAccountReference,
  TLimitRow extends ChatAccountReference,
>(
  accounts: readonly TAccount[] | null | undefined,
  limitRows: readonly TLimitRow[],
  missingRow: (account: TAccount) => TLimitRow,
): TLimitRow[] => {
  if (!accounts) return uniqueAccountsByHome(limitRows);
  const rowsById = new Map(limitRows.map((row) => [row.id, row]));

  return uniqueAccountsByHome(accounts).map((account) => {
    const row = rowsById.get(account.id);
    if (!row) return missingRow(account);
    return {
      ...row,
      id: account.id,
      label: account.label,
      provider: account.provider ?? "codex",
      codexHome: account.codexHome,
    };
  });
};
