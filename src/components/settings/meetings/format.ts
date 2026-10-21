import type { TFunction } from "i18next";
import type { MeetingRecord } from "@/bindings";

/** `m:ss`, or `h:mm:ss` past an hour. Matches the backend's timestamps. */
export const formatClock = (ms: number): string => {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, "0");
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${ss}`;
  return `${m}:${ss}`;
};

/** "42 min", "1 h 05", "< 1 min". */
export const formatDuration = (ms: number): string => {
  const minutes = Math.round(Math.max(0, ms) / 60000);
  if (minutes === 0) return "< 1 min";
  if (minutes < 60) return `${minutes} min`;
  return `${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, "0")}`;
};

export const formatTime = (epochMs: number, locale: string): string =>
  new Intl.DateTimeFormat(locale, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(epochMs));

export const formatLongDate = (epochMs: number, locale: string): string =>
  new Intl.DateTimeFormat(locale, {
    weekday: "long",
    day: "numeric",
    month: "long",
    year:
      new Date(epochMs).getFullYear() === new Date().getFullYear()
        ? undefined
        : "numeric",
  }).format(new Date(epochMs));

const startOfDay = (epochMs: number): number => {
  const d = new Date(epochMs);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};

/** "Today", "Yesterday" or the date, for grouping the list by day. */
export const dayLabel = (
  epochMs: number,
  locale: string,
  t: TFunction,
): string => {
  const today = startOfDay(Date.now());
  const day = startOfDay(epochMs);
  if (day === today) return t("meetings.list.today");
  if (day === today - 86_400_000) return t("meetings.list.yesterday");
  return formatLongDate(epochMs, locale);
};

/** The meeting's name, or "Meeting on <date>" when it has none. */
export const meetingTitle = (
  record: Pick<MeetingRecord, "title" | "started_at">,
  locale: string,
  t: TFunction,
): string => {
  const title = record.title?.trim();
  if (title) return title;
  const date = new Intl.DateTimeFormat(locale, {
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(record.started_at));
  return t("meetings.list.untitled", { date });
};

/** A user-facing message for a backend meeting error code. */
export const meetingErrorMessage = (code: string, t: TFunction): string => {
  const known = [
    "no_model",
    "microphone_permission_denied",
    "no_input_device",
    "already_recording",
    "still_finishing",
    "not_recording",
    "meeting_in_progress",
    "export_dir_missing",
  ];
  if (known.includes(code)) return t(`meetings.errors.${code}`);
  const detail = code.startsWith("unknown:") ? code.slice(8) : code;
  return t("meetings.errors.generic", { detail });
};
