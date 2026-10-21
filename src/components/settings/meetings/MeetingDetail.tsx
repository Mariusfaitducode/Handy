import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  Check,
  ChevronLeft,
  Copy,
  Download,
  Pencil,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";
import {
  commands,
  events,
  type MeetingDetail as Detail,
  type MeetingSegment,
  type Speaker,
} from "@/bindings";
import { useSettings } from "../../../hooks/useSettings";
import { copyToClipboard } from "../history/clipboard";
import { Input } from "../../ui/Input";
import {
  formatClock,
  formatDuration,
  formatLongDate,
  formatTime,
  meetingErrorMessage,
  meetingTitle,
} from "./format";
import { LiveMeetingCard } from "./LiveMeetingCard";
import { pickExportFolder } from "./MeetingSettingsGroup";
import { MeetingSummary } from "./MeetingSummary";
import { useMeetingStatus } from "./useMeetingStatus";

type Tab = "summary" | "transcript";

/** Request to show a section of the app (handled by App.tsx). */
export const OPEN_SECTION_EVENT = "handy-open-section";

const IconButton: React.FC<{
  onClick: () => void;
  title: string;
  disabled?: boolean;
  children: React.ReactNode;
}> = ({ onClick, title, disabled, children }) => (
  <button
    onClick={onClick}
    disabled={disabled}
    title={title}
    aria-label={title}
    className="p-1.5 rounded-md text-text/60 hover:text-text hover:bg-mid-gray/10 transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-40"
  >
    {children}
  </button>
);

interface Turn {
  key: number;
  speaker: Speaker;
  startMs: number;
  text: string;
}

/** Consecutive segments of one speaker read as one turn (as in exports). */
const toTurns = (segments: MeetingSegment[]): Turn[] => {
  const sorted = [...segments].sort(
    (a, b) => a.start_ms - b.start_ms || a.id - b.id,
  );
  const turns: (Turn & { endMs: number })[] = [];
  for (const segment of sorted) {
    const text = segment.text.trim();
    if (!text) continue;
    const last = turns[turns.length - 1];
    if (
      last &&
      last.speaker === segment.speaker &&
      segment.start_ms - last.endMs <= 4000
    ) {
      last.text += ` ${text}`;
      last.endMs = Math.max(last.endMs, segment.end_ms);
      continue;
    }
    turns.push({
      key: segment.id,
      speaker: segment.speaker,
      startMs: segment.start_ms,
      endMs: segment.end_ms,
      text,
    });
  }
  return turns;
};

export const MeetingDetail: React.FC<{ id: number; onBack: () => void }> = ({
  id,
  onBack,
}) => {
  const { t, i18n } = useTranslation();
  const { getSetting } = useSettings();
  const { status, elapsedMs } = useMeetingStatus();
  const [detail, setDetail] = useState<Detail | null>(null);
  const [missing, setMissing] = useState(false);
  const [tab, setTab] = useState<Tab | null>(null);
  const [copied, setCopied] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [draftTitle, setDraftTitle] = useState("");

  const load = useCallback(async () => {
    const result = await commands.getMeeting(id);
    if (result.status === "ok") {
      setDetail(result.data);
      setMissing(false);
    } else {
      setMissing(true);
    }
  }, [id]);

  useEffect(() => {
    load();
    const added = events.meetingSegmentEvent.listen((event) => {
      if (event.payload.meeting_id !== id) return;
      setDetail((prev) =>
        prev
          ? {
              ...prev,
              segments: [
                ...prev.segments.filter(
                  (s) => s.id !== event.payload.segment.id,
                ),
                event.payload.segment,
              ],
            }
          : prev,
      );
    });
    const removed = events.meetingSegmentRemovedEvent.listen((event) => {
      if (event.payload.meeting_id !== id) return;
      setDetail((prev) =>
        prev
          ? {
              ...prev,
              segments: prev.segments.filter(
                (s) => s.id !== event.payload.segment_id,
              ),
            }
          : prev,
      );
    });
    const updated = events.meetingUpdatedEvent.listen((event) => {
      if (event.payload.meeting_id !== id) return;
      if (event.payload.deleted) onBack();
      else load();
    });
    return () => {
      added.then((fn) => fn());
      removed.then((fn) => fn());
      updated.then((fn) => fn());
    };
  }, [id, load, onBack]);

  const live =
    status?.meeting_id === id &&
    (status.phase === "recording" || status.phase === "paused");
  const finishing = status?.meeting_id === id && status.phase === "finishing";
  const record = detail?.record;

  // Pick the tab once the meeting is known: the minutes when there are (or
  // will be) some, the transcript while it is being recorded.
  useEffect(() => {
    if (!record || tab !== null) return;
    const hasSummaryState =
      record.summary_status !== "none" || !!record.summary;
    setTab(live || !hasSummaryState ? "transcript" : "summary");
  }, [record, tab, live]);

  const turns = useMemo(() => toTurns(detail?.segments ?? []), [detail]);

  // Follow the live transcript while the reader is at its end.
  const endRef = useRef<HTMLDivElement>(null);
  const atEnd = useRef(true);
  useEffect(() => {
    const node = endRef.current;
    if (!node) return;
    const observer = new IntersectionObserver(
      (entries) => {
        atEnd.current = entries[0]?.isIntersecting ?? false;
      },
      { threshold: 0 },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [tab]);
  useEffect(() => {
    if (live && tab === "transcript" && atEnd.current) {
      endRef.current?.scrollIntoView({ block: "end", behavior: "smooth" });
    }
  }, [turns.length, live, tab]);

  if (missing) {
    return (
      <div className="max-w-3xl w-full mx-auto space-y-4">
        <BackButton onBack={onBack} />
        <p className="px-4 text-sm text-text/60">
          {t("meetings.detail.notFound")}
        </p>
      </div>
    );
  }
  if (!record) return null;

  const title = meetingTitle(record, i18n.language, t);
  const busy = live || finishing || record.status === "processing";

  const saveTitle = async () => {
    setRenaming(false);
    const next = draftTitle.trim();
    if (next === (record.title ?? "")) return;
    const result = await commands.renameMeeting(id, next || null);
    if (result.status === "error")
      toast.error(t("meetings.errors.generic", { detail: result.error }));
  };

  const copy = async () => {
    const part = tab === "summary" ? "summary" : "transcript";
    const result = await commands.getMeetingText(id, part);
    const ok = result.status === "ok" && (await copyToClipboard(result.data));
    if (!ok) {
      toast.error(t("meetings.detail.copyError"));
      return;
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1800);
  };

  const exportMarkdown = async () => {
    const current = getSetting("meeting")?.export_dir ?? null;
    let dir: string | null = null;
    if (!current) {
      dir = await pickExportFolder(t("meetings.detail.chooseFolder"), null);
      if (!dir) return;
    }
    let result = await commands.exportMeetingMarkdown(id, dir);
    if (result.status === "error" && result.error === "export_dir_missing") {
      dir = await pickExportFolder(t("meetings.detail.chooseFolder"), null);
      if (!dir) return;
      result = await commands.exportMeetingMarkdown(id, dir);
    }
    if (result.status === "error") {
      toast.error(meetingErrorMessage(result.error, t));
      return;
    }
    const path = result.data;
    const folder = path.split("/").slice(-2, -1)[0] ?? path;
    toast.success(t("meetings.detail.exported", { folder }), {
      action: {
        label: t("meetings.detail.reveal"),
        onClick: () => {
          commands.revealMeetingExport(path);
        },
      },
    });
  };

  const remove = async () => {
    const confirmed = await ask(t("meetings.detail.deleteConfirmBody"), {
      title: t("meetings.detail.deleteConfirm"),
      kind: "warning",
      okLabel: t("meetings.detail.delete"),
      cancelLabel: t("meetings.detail.cancel"),
    });
    if (!confirmed) return;
    const result = await commands.deleteMeeting(id);
    if (result.status === "error")
      toast.error(meetingErrorMessage(result.error, t));
    else onBack();
  };

  const generate = async () => {
    const result = await commands.regenerateMeetingSummary(id);
    if (result.status === "error")
      toast.error(meetingErrorMessage(result.error, t));
  };

  // Post-processing is switched on in Advanced; its provider is set up in
  // its own section, which only exists once it is on.
  const openSettings = () => {
    const section = getSetting("post_process_enabled")
      ? "postprocessing"
      : "advanced";
    window.dispatchEvent(
      new CustomEvent(OPEN_SECTION_EVENT, { detail: section }),
    );
  };

  const canCopy =
    tab === "summary"
      ? record.summary_status === "done" && !!record.summary
      : turns.length > 0;

  const meta = [
    formatLongDate(record.started_at, i18n.language),
    formatTime(record.started_at, i18n.language),
    live ? formatClock(elapsedMs) : formatDuration(record.duration_ms),
    record.system_audio === "captured"
      ? t("meetings.detail.withSystemAudio")
      : t("meetings.detail.micOnly"),
  ];

  return (
    <div className="max-w-3xl w-full mx-auto space-y-4">
      <BackButton onBack={onBack} />

      <div className="px-4 flex items-start gap-3">
        <div className="flex-1 min-w-0">
          {renaming ? (
            <Input
              autoFocus
              value={draftTitle}
              placeholder={t("meetings.detail.renamePlaceholder")}
              onChange={(e) => setDraftTitle(e.target.value)}
              onBlur={saveTitle}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveTitle();
                if (e.key === "Escape") setRenaming(false);
              }}
              className="w-full text-base"
              variant="compact"
              maxLength={120}
            />
          ) : (
            <button
              onClick={() => {
                setDraftTitle(record.title ?? "");
                setRenaming(true);
              }}
              title={t("meetings.detail.rename")}
              className="group text-start text-lg font-semibold leading-snug cursor-pointer break-words"
            >
              {title}
              <Pencil
                aria-hidden
                className="inline-block w-3.5 h-3.5 ms-2 align-baseline text-mid-gray opacity-0 group-hover:opacity-100 transition-opacity"
              />
            </button>
          )}
          <p className="text-xs text-mid-gray mt-0.5 first-letter:uppercase tabular-nums">
            {meta.join(" · ")}
          </p>
        </div>
        <div className="flex items-center shrink-0">
          <IconButton
            onClick={copy}
            disabled={!canCopy}
            title={
              copied
                ? t("meetings.detail.copied")
                : tab === "summary"
                  ? t("meetings.detail.copySummary")
                  : t("meetings.detail.copyTranscript")
            }
          >
            {copied ? <Check size={16} /> : <Copy size={16} />}
          </IconButton>
          <IconButton
            onClick={exportMarkdown}
            disabled={busy}
            title={t("meetings.detail.export")}
          >
            <Download size={16} />
          </IconButton>
          <IconButton
            onClick={remove}
            disabled={busy}
            title={t("meetings.detail.delete")}
          >
            <Trash2 size={16} />
          </IconButton>
        </div>
      </div>

      {(live || finishing) && status && (
        <div className="bg-background border border-mid-gray/20 rounded-lg">
          <LiveMeetingCard status={status} elapsedMs={elapsedMs} />
        </div>
      )}

      {record.status === "interrupted" && (
        <p className="px-4 text-xs text-text/70">
          {t("meetings.detail.interrupted")}
        </p>
      )}

      <div className="px-4 flex gap-4 border-b border-mid-gray/20">
        {(["summary", "transcript"] as Tab[]).map((value) => (
          <button
            key={value}
            onClick={() => setTab(value)}
            className={`-mb-px pb-2 text-sm cursor-pointer border-b-2 transition-colors ${
              tab === value
                ? "border-logo-primary text-text font-medium"
                : "border-transparent text-text/60 hover:text-text"
            }`}
          >
            {t(`meetings.detail.tabs.${value}`)}
          </button>
        ))}
      </div>

      <div className="px-4 pb-4">
        {tab === "summary" ? (
          <MeetingSummary
            record={record}
            live={live || finishing}
            onGenerate={generate}
            onOpenSettings={openSettings}
          />
        ) : turns.length === 0 ? (
          <p className="text-sm text-text/60 py-1 leading-relaxed">
            {live
              ? t("meetings.detail.transcriptWaiting")
              : t("meetings.detail.transcriptEmpty")}
          </p>
        ) : (
          <ol className="space-y-4">
            {turns.map((turn) => (
              <li key={turn.key} className="flex gap-3">
                <span className="w-12 shrink-0 text-xs text-mid-gray tabular-nums pt-0.5 text-end">
                  {formatClock(turn.startMs)}
                </span>
                <div className="min-w-0">
                  <p
                    className={`text-xs font-medium ${
                      turn.speaker === "me" ? "text-text" : "text-text/60"
                    }`}
                  >
                    {t(`meetings.detail.speakers.${turn.speaker}`)}
                  </p>
                  <p className="text-sm leading-relaxed text-text/90 select-text cursor-text whitespace-pre-wrap break-words">
                    {turn.text}
                  </p>
                </div>
              </li>
            ))}
          </ol>
        )}
        <div ref={endRef} className="h-1" />
      </div>
    </div>
  );
};

const BackButton: React.FC<{ onBack: () => void }> = ({ onBack }) => {
  const { t } = useTranslation();
  return (
    <button
      onClick={onBack}
      className="px-3 flex items-center gap-1 text-sm text-text/60 hover:text-text cursor-pointer"
    >
      <ChevronLeft className="w-4 h-4 rtl:rotate-180" />
      {t("meetings.detail.back")}
    </button>
  );
};
