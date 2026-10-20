import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Pause, Play, Square } from "lucide-react";
import { toast } from "sonner";
import { commands, type MeetingStatus } from "@/bindings";
import { Button } from "../../ui/Button";
import { formatClock, meetingErrorMessage } from "./format";
import { SystemAudioNotice } from "./SystemAudioNotice";

interface LiveMeetingCardProps {
  status: MeetingStatus;
  elapsedMs: number;
  /** Shown as a link when the card is outside the meeting's own page. */
  onShowTranscript?: () => void;
}

/** The meeting in progress: elapsed time, pause, stop. */
export const LiveMeetingCard: React.FC<LiveMeetingCardProps> = ({
  status,
  elapsedMs,
  onShowTranscript,
}) => {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);

  const run = async (
    action: () => Promise<{ status: string; error?: string }>,
  ) => {
    setBusy(true);
    try {
      const result = await action();
      if (result.status === "error" && result.error) {
        toast.error(meetingErrorMessage(result.error, t));
      }
    } finally {
      setBusy(false);
    }
  };

  if (status.phase === "finishing") {
    const count = status.pending_utterances;
    return (
      <div className="flex items-center gap-3 px-4 py-3">
        <div className="w-3.5 h-3.5 border-2 border-mid-gray/60 border-t-transparent rounded-full animate-spin" />
        <p className="text-sm text-text/80">
          {count > 0
            ? t("meetings.live.finishingCount", { count })
            : t("meetings.live.finishing")}
        </p>
      </div>
    );
  }

  const paused = status.phase === "paused";
  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <div className="flex items-center gap-3">
        <span className="relative flex h-2.5 w-2.5 shrink-0">
          {!paused && (
            <span className="absolute inline-flex h-full w-full rounded-full bg-red-500 opacity-60 animate-ping" />
          )}
          <span
            className={`relative inline-flex h-2.5 w-2.5 rounded-full ${
              paused ? "bg-mid-gray" : "bg-red-500"
            }`}
          />
        </span>
        <div className="flex-1 min-w-0">
          <p className="text-sm font-medium truncate">
            {status.title || t("meetings.live.untitled")}
          </p>
          <p className="text-xs text-mid-gray tabular-nums">
            {paused ? t("meetings.live.paused") : t("meetings.live.recording")}{" "}
            · {formatClock(elapsedMs)}
          </p>
        </div>
        <Button
          variant="secondary"
          size="sm"
          disabled={busy}
          onClick={() =>
            run(paused ? commands.resumeMeeting : commands.pauseMeeting)
          }
          className="flex items-center gap-1.5"
        >
          {paused ? (
            <Play className="w-3.5 h-3.5" />
          ) : (
            <Pause className="w-3.5 h-3.5" />
          )}
          {paused ? t("meetings.live.resume") : t("meetings.live.pause")}
        </Button>
        <Button
          variant="primary"
          size="sm"
          disabled={busy}
          onClick={() => run(commands.stopMeeting)}
          className="flex items-center gap-1.5"
        >
          <Square className="w-3 h-3" fill="currentColor" />
          {t("meetings.live.stop")}
        </Button>
      </div>
      {status.system_audio && status.system_audio !== "captured" && (
        <SystemAudioNotice state={status.system_audio} />
      )}
      {onShowTranscript && (
        <button
          onClick={onShowTranscript}
          className="self-start text-xs text-mid-gray hover:text-text underline underline-offset-2 cursor-pointer"
        >
          {t("meetings.live.showTranscript")}
        </button>
      )}
    </div>
  );
};
