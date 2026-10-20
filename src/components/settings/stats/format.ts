/** Pure formatting helpers for the Statistics page (kept free of React so
 * they can be unit tested with `bun`). */

export type DurationParts =
  | { kind: "lessThanMinute" }
  | { kind: "minutes"; minutes: number }
  | { kind: "hours"; hours: number }
  | { kind: "hoursMinutes"; hours: number; minutes: number };

/** Split a duration into the parts a person reads: minutes under an hour,
 * hours and minutes under 10 hours, whole hours beyond (minutes stop
 * mattering). */
export const durationParts = (totalSeconds: number): DurationParts => {
  const seconds = Math.max(0, Math.round(totalSeconds));
  if (seconds < 60) return { kind: "lessThanMinute" };
  const totalMinutes = Math.round(seconds / 60);
  if (totalMinutes < 60) return { kind: "minutes", minutes: totalMinutes };
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours >= 10 || minutes === 0) {
    return { kind: "hours", hours: Math.round(totalMinutes / 60) };
  }
  return { kind: "hoursMinutes", hours, minutes };
};

/** Parse a `YYYY-MM-DD` local day without the UTC shift `new Date(str)`
 * would apply. */
export const parseLocalDay = (day: string): Date | null => {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!match) return null;
  return new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
};

/** Share of `part` in `total`, as a whole percentage. Small non-zero shares
 * show as 1% rather than a misleading 0%. */
export const percentage = (part: number, total: number): number => {
  if (total <= 0 || part <= 0) return 0;
  const value = Math.round((part / total) * 100);
  return Math.min(100, Math.max(1, value));
};

/** Bar height in percent of the chart, with a visible minimum for days that
 * have any activity. */
export const barHeight = (words: number, max: number): number => {
  if (words <= 0 || max <= 0) return 0;
  return Math.max(6, Math.round((words / max) * 100));
};

/** Capitalized display name of a language code in the UI locale, falling back
 * to the code itself. */
export const languageName = (code: string, locale: string): string => {
  try {
    const name = new Intl.DisplayNames([locale], { type: "language" }).of(code);
    if (!name || name === code) return code.toUpperCase();
    return name.charAt(0).toLocaleUpperCase(locale) + name.slice(1);
  } catch {
    return code.toUpperCase();
  }
};
