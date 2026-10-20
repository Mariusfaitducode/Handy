import type { DestinationState, VoiceNote } from "@/bindings";

/** Lowercase and strip accents so "ete" finds "été". */
export const normalizeForSearch = (text: string): string =>
  text
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .toLowerCase();

/** Every whitespace-separated term must appear in the title or the body. */
export const matchesQuery = (note: VoiceNote, query: string): boolean => {
  const terms = normalizeForSearch(query).split(/\s+/).filter(Boolean);
  if (terms.length === 0) return true;
  const haystack = normalizeForSearch(`${note.title}\n${note.body}`);
  return terms.every((term) => haystack.includes(term));
};

const dayKey = (date: Date): string =>
  `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;

export interface NoteDayGroup {
  key: string;
  /** Midnight of that day, local time. */
  day: Date;
  notes: VoiceNote[];
}

/** Group notes (already newest first) by local calendar day. */
export const groupByDay = (notes: VoiceNote[]): NoteDayGroup[] => {
  const groups: NoteDayGroup[] = [];
  for (const note of notes) {
    const date = new Date(note.created_at * 1000);
    const key = dayKey(date);
    const last = groups[groups.length - 1];
    if (last && last.key === key) {
      last.notes.push(note);
    } else {
      const day = new Date(date);
      day.setHours(0, 0, 0, 0);
      groups.push({ key, day, notes: [note] });
    }
  }
  return groups;
};

export type DayLabel =
  | { kind: "today" }
  | { kind: "yesterday" }
  | { kind: "date"; sameYear: boolean };

export const dayLabel = (day: Date, now: Date = new Date()): DayLabel => {
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const diffDays = Math.round((today.getTime() - day.getTime()) / 86_400_000);
  if (diffDays === 0) return { kind: "today" };
  if (diffDays === 1) return { kind: "yesterday" };
  return { kind: "date", sameYear: day.getFullYear() === today.getFullYear() };
};

/** Error codes are `code` or `code: detail`; return the bare code. */
export const errorCode = (error: string): string => error.split(":")[0].trim();

const KNOWN_ERRORS = new Set([
  "permission_denied",
  "io",
  "notion_invalid_token",
  "notion_page_not_found",
  "notion_invalid_page_link",
  "notion_network",
  "notion_api",
  "notion_token_missing",
  "notion_keychain",
  "apple_notes_denied",
  "apple_notes_failed",
]);

/** i18n key for a backend error code, falling back to a generic message. */
export const errorKey = (error: string): string => {
  const code = errorCode(error);
  return KNOWN_ERRORS.has(code)
    ? `notes.errors.${code}`
    : "notes.errors.generic";
};

export type DestinationId = "markdown" | "notion" | "apple_notes";

export const DESTINATIONS: DestinationId[] = [
  "markdown",
  "notion",
  "apple_notes",
];

export const destinationState = (
  note: VoiceNote,
  id: DestinationId,
): DestinationState => note[id];

export const hasFailedDestination = (note: VoiceNote): boolean =>
  DESTINATIONS.some((id) => note[id].status === "failed");

/** Text placed on the clipboard: title, blank line, body. */
export const clipboardText = (note: VoiceNote): string =>
  note.title ? `${note.title}\n\n${note.body}` : note.body;
