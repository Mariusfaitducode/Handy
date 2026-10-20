import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, Users } from "lucide-react";
import { toast } from "sonner";
import {
  commands,
  events,
  type MeetingCapabilities,
  type MeetingRecord,
} from "@/bindings";
import { useSettings } from "../../../hooks/useSettings";
import { useOsType } from "../../../hooks/useOsType";
import { formatKeyCombination } from "../../../lib/utils/keyboard";
import { useMeetingNavigation } from "../../../stores/meetingNavigationStore";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import {
  dayLabel,
  formatDuration,
  formatTime,
  meetingErrorMessage,
  meetingTitle,
} from "./format";
import { LiveMeetingCard } from "./LiveMeetingCard";
import { MeetingDetail } from "./MeetingDetail";
import { MeetingSettingsGroup } from "./MeetingSettingsGroup";
import { SystemAudioNotice } from "./SystemAudioNotice";
import { useMeetingStatus } from "./useMeetingStatus";

export const MeetingsSettings: React.FC = () => {
  const openMeetingId = useMeetingNavigation((s) => s.openMeetingId);
  const openMeeting = useMeetingNavigation((s) => s.openMeeting);
  const back = useCallback(() => openMeeting(null), [openMeeting]);

  // Coming back to the section shows the list, not the last meeting opened.
  useEffect(() => () => openMeeting(null), [openMeeting]);

  if (openMeetingId !== null) {
    return <MeetingDetail id={openMeetingId} onBack={back} />;
  }
  return <MeetingsHome onOpen={openMeeting} />;
};

/** Meetings are reloaded whenever one changes; there are few of them. */
const useMeetings = () => {
  const [meetings, setMeetings] = useState<MeetingRecord[] | null>(null);
  const [failed, setFailed] = useState(false);

  const reload = useCallback(async () => {
    const result = await commands.listMeetings();
    if (result.status === "ok") {
      setMeetings(result.data);
      setFailed(false);
    } else {
      setFailed(true);
    }
  }, []);

  useEffect(() => {
    reload();
    const updated = events.meetingUpdatedEvent.listen(() => reload());
    return () => {
      updated.then((fn) => fn());
    };
  }, [reload]);

  return { meetings, failed };
};

const MeetingsHome: React.FC<{ onOpen: (id: number) => void }> = ({
  onOpen,
}) => {
  const { t, i18n } = useTranslation();
  const { status, elapsedMs } = useMeetingStatus();
  const { meetings, failed } = useMeetings();
  const [capabilities, setCapabilities] = useState<MeetingCapabilities | null>(
    null,
  );

  useEffect(() => {
    commands
      .getMeetingCapabilities()
      .then(setCapabilities)
      .catch(() => {});
  }, [status?.phase]);

  const groups = useMemo(() => {
    const out: { label: string; items: MeetingRecord[] }[] = [];
    for (const meeting of meetings ?? []) {
      const label = dayLabel(meeting.started_at, i18n.language, t);
      const last = out[out.length - 1];
      if (last && last.label === label) last.items.push(meeting);
      else out.push({ label, items: [meeting] });
    }
    return out;
  }, [meetings, i18n.language, t]);

  const active = status && status.phase !== "idle" ? status : null;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="space-y-2">
        <div className="px-4">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("meetings.title")}
          </h2>
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg">
          {active ? (
            <LiveMeetingCard
              status={active}
              elapsedMs={elapsedMs}
              onShowTranscript={
                active.meeting_id !== null
                  ? () => onOpen(active.meeting_id as number)
                  : undefined
              }
            />
          ) : (
            <StartMeeting capabilities={capabilities} />
          )}
        </div>
      </div>

      {failed ? (
        <p className="px-4 text-sm text-text/60">
          {t("meetings.list.loadError")}
        </p>
      ) : meetings === null ? null : meetings.length === 0 ? (
        <EmptyState />
      ) : (
        groups.map((group) => (
          <div key={group.label} className="space-y-2">
            <div className="px-4">
              <h3 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                {group.label}
              </h3>
            </div>
            <div className="bg-background border border-mid-gray/20 rounded-lg divide-y divide-mid-gray/20">
              {group.items.map((meeting) => (
                <MeetingRow
                  key={meeting.id}
                  meeting={meeting}
                  onOpen={() => onOpen(meeting.id)}
                />
              ))}
            </div>
          </div>
        ))
      )}

      <MeetingSettingsGroup capabilities={capabilities} />

      <p className="px-4 text-xs text-mid-gray">{t("meetings.privacy")}</p>
    </div>
  );
};

const StartMeeting: React.FC<{ capabilities: MeetingCapabilities | null }> = ({
  capabilities,
}) => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const osType = useOsType();
  const [title, setTitle] = useState("");
  const [starting, setStarting] = useState(false);

  const shortcut = getSetting("bindings")?.toggle_meeting?.current_binding;

  const start = async () => {
    setStarting(true);
    try {
      const result = await commands.startMeeting(title.trim() || null);
      if (result.status === "error") {
        toast.error(meetingErrorMessage(result.error, t));
      } else {
        setTitle("");
      }
    } finally {
      setStarting(false);
    }
  };

  const supported = capabilities?.system_audio_supported ?? true;
  const permission = capabilities?.system_audio_permission;

  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <form
        className="flex items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (!starting) start();
        }}
      >
        <Input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder={t("meetings.start.titlePlaceholder")}
          className="flex-1 min-w-0 font-normal"
          variant="compact"
          maxLength={120}
        />
        <Button
          type="submit"
          variant="primary"
          size="md"
          disabled={starting}
          className="flex items-center gap-2 shrink-0"
        >
          <span className="inline-block w-2 h-2 rounded-full bg-white" />
          {starting ? t("meetings.start.starting") : t("meetings.start.button")}
        </Button>
      </form>
      <p className="text-xs text-mid-gray leading-relaxed">
        {supported ? t("meetings.start.hint") : t("meetings.start.hintMicOnly")}
        {shortcut && (
          <>
            {" "}
            {t("meetings.start.shortcut", {
              shortcut: formatKeyCombination(shortcut, osType),
            })}
          </>
        )}
      </p>
      {supported && permission === "denied" && (
        <SystemAudioNotice state="denied" />
      )}
      {supported && permission === "unknown" && (
        <p className="text-xs text-mid-gray leading-relaxed">
          {t("meetings.systemAudio.promptHint")}
        </p>
      )}
    </div>
  );
};

const EmptyState: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col items-center text-center gap-2 px-8 py-8">
      <Users className="w-6 h-6 text-mid-gray" strokeWidth={1.5} />
      <p className="text-sm font-medium">{t("meetings.list.emptyTitle")}</p>
      <p className="text-sm text-text/60 max-w-sm leading-relaxed">
        {t("meetings.list.emptyBody")}
      </p>
    </div>
  );
};

const statusLabel = (meeting: MeetingRecord): string => {
  if (meeting.status === "recording") return "recording";
  if (meeting.status === "processing") return "processing";
  if (meeting.summary_status === "pending") return "summarizing";
  if (meeting.status === "interrupted") return "interrupted";
  if (meeting.summary_status === "done") return "summary";
  if (meeting.summary_status === "failed") return "failed";
  return "transcript";
};

const MeetingRow: React.FC<{ meeting: MeetingRecord; onOpen: () => void }> = ({
  meeting,
  onOpen,
}) => {
  const { t, i18n } = useTranslation();
  const label = statusLabel(meeting);
  const live = label === "recording";
  return (
    <button
      onClick={onOpen}
      className="w-full flex items-center gap-3 px-4 py-2.5 text-start hover:bg-mid-gray/10 transition-colors cursor-pointer first:rounded-t-lg last:rounded-b-lg"
    >
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium truncate">
          {meetingTitle(meeting, i18n.language, t)}
        </p>
        <p className="text-xs text-mid-gray tabular-nums">
          {formatTime(meeting.started_at, i18n.language)}
          {!live && <> · {formatDuration(meeting.duration_ms)}</>}
        </p>
      </div>
      <span
        className={`text-xs shrink-0 ${
          live ? "text-red-500" : "text-mid-gray"
        }`}
      >
        {t(`meetings.list.status.${label}`)}
      </span>
      <ChevronRight className="w-4 h-4 text-mid-gray shrink-0 rtl:rotate-180" />
    </button>
  );
};
