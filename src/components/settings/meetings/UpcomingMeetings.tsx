import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { CalendarDays } from "lucide-react";
import { toast } from "sonner";
import {
  commands,
  events,
  type CalendarOverview,
  type UpcomingMeeting,
} from "@/bindings";
import { Button } from "../../ui/Button";
import { formatTime } from "./format";

/** The calendar overview, refetched whenever the backend says it changed. */
export const useCalendarOverview = () => {
  const [overview, setOverview] = useState<CalendarOverview | null>(null);

  const reload = useCallback(async () => {
    const result = await commands.getCalendarOverview();
    if (result.status === "ok") setOverview(result.data);
  }, []);

  useEffect(() => {
    reload();
    const changed = events.calendarChangedEvent.listen(() => reload());
    // "Now" and "Tomorrow" labels move with the clock.
    const id = window.setInterval(reload, 60_000);
    window.addEventListener("focus", reload);
    return () => {
      changed.then((fn) => fn());
      window.clearInterval(id);
      window.removeEventListener("focus", reload);
    };
  }, [reload]);

  return { overview, setOverview, reload };
};

const PROVIDERS: Record<string, string> = {
  meet: "Google Meet",
  zoom: "Zoom",
  teams: "Teams",
  webex: "Webex",
  facetime: "FaceTime",
  whereby: "Whereby",
  jitsi: "Jitsi",
  around: "Around",
  slack: "Slack",
};

const isTomorrow = (epochMs: number) => {
  const d = new Date(epochMs);
  const tomorrow = new Date();
  tomorrow.setDate(tomorrow.getDate() + 1);
  return d.toDateString() === tomorrow.toDateString();
};

/**
 * "Upcoming meetings", at the top of the Meetings section: the next video
 * calls from the calendar and what Handy will do about each, or the one
 * step needed to get there (connect, or grant access again).
 */
export const UpcomingMeetings: React.FC<{
  overview: CalendarOverview | null;
  onChange: (overview: CalendarOverview) => void;
}> = ({ overview, onChange }) => {
  const { t } = useTranslation();
  const [connecting, setConnecting] = useState(false);

  if (!overview || overview.access === "unsupported") return null;
  // Access granted but turned off on purpose: nothing to nag about.
  if (overview.access === "granted" && overview.mode === "off") return null;

  const connect = async () => {
    setConnecting(true);
    try {
      const result = await commands.enableCalendar();
      if (result.status === "ok") onChange(result.data);
      else toast.error(t("meetings.calendar.error"));
    } finally {
      setConnecting(false);
    }
  };

  const openSettings = (pane: "calendars" | "notifications") =>
    commands.openCalendarSystemSettings(pane);

  let body: React.ReactNode;
  if (overview.access === "not_determined") {
    body = (
      <div className="flex items-center gap-3 px-4 py-3">
        <CalendarDays
          className="w-5 h-5 text-mid-gray shrink-0"
          strokeWidth={1.5}
        />
        <p className="flex-1 text-sm text-text/70 leading-relaxed">
          {t("meetings.calendar.connect.body")}
        </p>
        <Button
          variant="secondary"
          size="sm"
          disabled={connecting}
          onClick={connect}
          className="shrink-0"
        >
          {connecting
            ? t("meetings.calendar.connect.connecting")
            : t("meetings.calendar.connect.button")}
        </Button>
      </div>
    );
  } else if (overview.access === "denied") {
    body = (
      <p className="px-4 py-3 text-sm text-text/70 leading-relaxed">
        {t("meetings.calendar.denied")}{" "}
        <button
          onClick={() => openSettings("calendars")}
          className="underline underline-offset-2 hover:text-text cursor-pointer"
        >
          {t("meetings.calendar.openSettings")}
        </button>
      </p>
    );
  } else {
    body = (
      <>
        {overview.upcoming.length === 0 ? (
          <p className="px-4 py-3 text-sm text-mid-gray">
            {t("meetings.calendar.empty")}
          </p>
        ) : (
          <div className="divide-y divide-mid-gray/20">
            {overview.upcoming.map((meeting) => (
              <UpcomingRow
                key={meeting.key}
                meeting={meeting}
                auto={overview.mode === "auto"}
              />
            ))}
          </div>
        )}
        <NotificationNotice
          overview={overview}
          onChange={onChange}
          onOpenSettings={() => openSettings("notifications")}
        />
      </>
    );
  }

  return (
    <div className="space-y-2">
      <div className="px-4">
        <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
          {t("meetings.calendar.title")}
        </h2>
      </div>
      <div className="bg-background border border-mid-gray/20 rounded-lg">
        {body}
      </div>
    </div>
  );
};

/** Prompts need notifications: say so when they're off. */
const NotificationNotice: React.FC<{
  overview: CalendarOverview;
  onChange: (overview: CalendarOverview) => void;
  onOpenSettings: () => void;
}> = ({ overview, onChange, onOpenSettings }) => {
  const { t } = useTranslation();
  const linkClass =
    "underline underline-offset-2 hover:text-text cursor-pointer";
  let content: React.ReactNode = null;
  if (overview.notifications === "denied") {
    content = (
      <>
        {t("meetings.calendar.notifications.denied")}{" "}
        <button onClick={onOpenSettings} className={linkClass}>
          {t("meetings.calendar.openSettings")}
        </button>
      </>
    );
  } else if (overview.notifications === "not_determined") {
    content = (
      <>
        {t("meetings.calendar.notifications.notDetermined")}{" "}
        <button
          onClick={async () => {
            const result = await commands.requestCalendarNotifications();
            if (result.status === "ok") onChange(result.data);
          }}
          className={linkClass}
        >
          {t("meetings.calendar.notifications.allow")}
        </button>
      </>
    );
  } else if (overview.notifications === "unavailable") {
    content = t("meetings.calendar.notifications.unavailable");
  }
  if (!content) return null;
  return (
    <p className="px-4 py-2.5 border-t border-mid-gray/20 text-xs text-mid-gray leading-relaxed">
      {content}
    </p>
  );
};

const UpcomingRow: React.FC<{ meeting: UpcomingMeeting; auto: boolean }> = ({
  meeting,
  auto,
}) => {
  const { t, i18n } = useTranslation();
  const [busy, setBusy] = useState(false);
  const now = Date.now();
  const inProgress = meeting.start_ms <= now && now < meeting.end_ms;
  const skipped = meeting.decision !== "record";

  const choose = async (record: boolean | null) => {
    setBusy(true);
    try {
      const result = await commands.setCalendarEventChoice(meeting.key, record);
      if (result.status === "error") toast.error(t("meetings.calendar.error"));
    } finally {
      setBusy(false);
    }
  };

  const when = inProgress
    ? t("meetings.calendar.now")
    : `${isTomorrow(meeting.start_ms) ? `${t("meetings.calendar.tomorrow")} ` : ""}${formatTime(meeting.start_ms, i18n.language)}`;

  const details = [
    meeting.video ? (PROVIDERS[meeting.video.provider] ?? null) : null,
    meeting.participants.length > 0
      ? meeting.participants.length > 2
        ? t("meetings.calendar.withMany", {
            names: meeting.participants.slice(0, 2).join(", "),
            count: meeting.participants.length - 2,
          })
        : t("meetings.calendar.with", {
            names: meeting.participants.join(", "),
          })
      : null,
  ].filter(Boolean);

  let status: string;
  if (meeting.recording) status = t("meetings.calendar.status.recording");
  else if (meeting.decision === "skipped")
    status = t("meetings.calendar.status.skipped");
  else if (meeting.decision === "no_attendees")
    status = t("meetings.calendar.status.noAttendees");
  else
    status = auto
      ? t("meetings.calendar.status.auto")
      : t("meetings.calendar.status.ask");

  const action = meeting.recording ? null : meeting.decision === "record" ? (
    <ActionLink disabled={busy} onClick={() => choose(false)}>
      {t("meetings.calendar.skip")}
    </ActionLink>
  ) : meeting.decision === "skipped" ? (
    <ActionLink disabled={busy} onClick={() => choose(null)}>
      {t("meetings.calendar.restore")}
    </ActionLink>
  ) : (
    <ActionLink disabled={busy} onClick={() => choose(true)}>
      {t("meetings.calendar.record")}
    </ActionLink>
  );

  return (
    <div
      className={`flex items-center gap-3 px-4 py-2.5 ${skipped ? "opacity-60" : ""}`}
    >
      <span
        className="w-1 self-stretch rounded-full shrink-0"
        style={{
          backgroundColor:
            meeting.calendar_color ?? "var(--color-mid-gray, #808080)",
        }}
        title={meeting.calendar_title}
        aria-hidden
      />
      <div className="w-20 shrink-0 text-xs text-mid-gray tabular-nums">
        {when}
      </div>
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium truncate">
          {meeting.title || t("meetings.calendar.untitled")}
        </p>
        {details.length > 0 && (
          <p className="text-xs text-mid-gray truncate">
            {details.join(" · ")}
          </p>
        )}
      </div>
      <span
        className={`text-xs shrink-0 ${
          meeting.recording ? "text-red-500" : "text-mid-gray"
        }`}
      >
        {status}
      </span>
      {action}
      {meeting.video && (
        <ActionLink
          onClick={() => commands.openMeetingLink(meeting.video!.url)}
        >
          {t("meetings.calendar.join")}
        </ActionLink>
      )}
    </div>
  );
};

const ActionLink: React.FC<{
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}> = ({ onClick, disabled, children }) => (
  <button
    onClick={onClick}
    disabled={disabled}
    className="text-xs text-text/70 hover:text-text underline underline-offset-2 cursor-pointer shrink-0 disabled:opacity-50"
  >
    {children}
  </button>
);
