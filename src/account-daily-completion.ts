export type DailyAccountCompletion = {
  completedOn?: string | null;
};

/** Cle de jour civil dans le fuseau local de l'appareil (YYYY-MM-DD). */
export const localCalendarDay = (date = new Date()): string => {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
};

export const accountCompletedToday = (
  account: DailyAccountCompletion,
  now = new Date(),
): boolean => account.completedOn === localCalendarDay(now);

/** Delai jusqu'au prochain minuit local, y compris les jours de changement d'heure. */
export const millisecondsUntilNextLocalMidnight = (now = new Date()): number => {
  const nextMidnight = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate() + 1,
  );
  return Math.max(1, nextMidnight.getTime() - now.getTime());
};
