import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { emit } from "@tauri-apps/api/event";
import { Pause, Play, Square } from "lucide-react";
import { commands } from "@/bindings";
import { formatClock } from "@/components/settings/meetings/format";
import { useMeetingStatus } from "@/components/settings/meetings/useMeetingStatus";
import { OPEN_MEETING_EVENT } from "@/stores/meetingNavigationStore";

/** The floating pill: elapsed time (opens the meeting), pause, stop. */
const MeetingIndicator: React.FC = () => {
  const { t } = useTranslation();
  const { status, elapsedMs } = useMeetingStatus();
  const [busy, setBusy] = useState(false);

  if (!status || (status.phase !== "recording" && status.phase !== "paused")) {
    return null;
  }
  const paused = status.phase === "paused";

  const act = async (action: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await action();
    } finally {
      setBusy(false);
    }
  };

  const open = async () => {
    await commands.showMainWindowCommand();
    if (status.meeting_id !== null) {
      await emit(OPEN_MEETING_EVENT, status.meeting_id);
    }
  };

  return (
    <div className="mi-root">
      <div
        className="mi-pill"
        role="status"
        aria-label={
          paused
            ? t("meetings.indicator.paused")
            : t("meetings.indicator.recording")
        }
      >
        <button
          className="mi-time"
          onClick={open}
          title={t("meetings.indicator.open")}
        >
          <span className={`mi-dot ${paused ? "paused" : ""}`} />
          {formatClock(elapsedMs)}
        </button>
        <button
          className="mi-button"
          disabled={busy}
          onClick={() =>
            act(paused ? commands.resumeMeeting : commands.pauseMeeting)
          }
          title={
            paused
              ? t("meetings.indicator.resume")
              : t("meetings.indicator.pause")
          }
          aria-label={
            paused
              ? t("meetings.indicator.resume")
              : t("meetings.indicator.pause")
          }
        >
          {paused ? (
            <Play size={14} fill="currentColor" />
          ) : (
            <Pause size={14} fill="currentColor" />
          )}
        </button>
        <button
          className="mi-button"
          disabled={busy}
          onClick={() => act(commands.stopMeeting)}
          title={t("meetings.indicator.stop")}
          aria-label={t("meetings.indicator.stop")}
        >
          <Square size={12} fill="currentColor" />
        </button>
      </div>
    </div>
  );
};

export default MeetingIndicator;
