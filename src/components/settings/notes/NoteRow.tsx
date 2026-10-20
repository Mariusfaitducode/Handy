import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  Check,
  CircleAlert,
  Copy,
  ExternalLink,
  RefreshCw,
  Sparkles,
  Trash2,
} from "lucide-react";
import { commands, type VoiceNote } from "@/bindings";
import { copyToClipboard } from "../history/clipboard";
import {
  DESTINATIONS,
  clipboardText,
  errorKey,
  hasFailedDestination,
  type DestinationId,
} from "./noteUtils";

const IconButton: React.FC<{
  onClick: () => void;
  title: string;
  children: React.ReactNode;
}> = ({ onClick, title, children }) => (
  <button
    type="button"
    onClick={(e) => {
      e.stopPropagation();
      onClick();
    }}
    className="p-1.5 rounded-md text-text/50 hover:text-logo-primary transition-colors cursor-pointer"
    title={title}
    aria-label={title}
  >
    {children}
  </button>
);

interface NoteRowProps {
  note: VoiceNote;
  expanded: boolean;
  onToggle: () => void;
  onDelete: () => void;
}

export const NoteRow: React.FC<NoteRowProps> = ({
  note,
  expanded,
  onToggle,
  onDelete,
}) => {
  const { t, i18n } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [showOriginal, setShowOriginal] = useState(false);
  const failed = hasFailedDestination(note);
  const canOpen = DESTINATIONS.some((id) => note[id].status === "done");

  const time = new Intl.DateTimeFormat(i18n.language, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(note.created_at * 1000));

  const handleCopy = async () => {
    const ok = await copyToClipboard(clipboardText(note));
    if (!ok) {
      toast.error(t("notes.toasts.copyError"));
      return;
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  };

  const handleOpen = async () => {
    const result = await commands.openVoiceNote(note.id);
    if (result.status !== "ok") toast.error(t("notes.toasts.openError"));
  };

  const openDestination = async (id: DestinationId) => {
    const result = await commands.openVoiceNoteDestination(note.id, id);
    if (result.status !== "ok") toast.error(t("notes.toasts.openError"));
  };

  const retry = async () => {
    const result = await commands.retryVoiceNoteExports(note.id);
    if (result.status !== "ok") toast.error(t("notes.toasts.retryError"));
  };

  const enabledDestinations = DESTINATIONS.filter(
    (id) => note[id].status !== "off",
  );

  return (
    <div
      className={`group px-4 py-2.5 transition-colors ${
        expanded ? "bg-mid-gray/5" : "hover:bg-mid-gray/5 cursor-pointer"
      }`}
      onClick={expanded ? undefined : onToggle}
    >
      <div className="flex items-center gap-3">
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onToggle();
          }}
          aria-expanded={expanded}
          title={expanded ? t("notes.row.collapse") : t("notes.row.expand")}
          className="flex-1 min-w-0 text-start cursor-pointer focus:outline-none focus-visible:underline"
        >
          <span className="block text-sm font-medium truncate">
            {note.title || note.body}
          </span>
        </button>
        <div className="flex items-center shrink-0">
          <div
            className={`flex items-center transition-opacity ${
              expanded
                ? "opacity-100"
                : "opacity-0 group-hover:opacity-100 focus-within:opacity-100"
            }`}
          >
            <IconButton onClick={handleCopy} title={t("notes.actions.copy")}>
              {copied ? (
                <Check width={15} height={15} />
              ) : (
                <Copy width={15} height={15} />
              )}
            </IconButton>
            {canOpen && (
              <IconButton onClick={handleOpen} title={t("notes.actions.open")}>
                <ExternalLink width={15} height={15} />
              </IconButton>
            )}
            <IconButton onClick={onDelete} title={t("notes.actions.delete")}>
              <Trash2 width={15} height={15} />
            </IconButton>
          </div>
          {failed && !expanded && (
            <CircleAlert
              className="w-3.5 h-3.5 ms-1 text-red-400"
              aria-label={t("notes.row.exportProblem")}
            />
          )}
          <span className="ms-2 w-11 text-end text-xs text-mid-gray tabular-nums">
            {time}
          </span>
        </div>
      </div>

      {note.title && (
        <p
          className={`mt-0.5 text-sm text-text/60 ${
            expanded
              ? "text-text/85 whitespace-pre-wrap break-words select-text cursor-text"
              : "line-clamp-2"
          }`}
        >
          {note.body}
        </p>
      )}

      {expanded && (
        <div className="mt-3 space-y-2">
          {note.ai_processed && note.raw_text !== note.body && (
            <div>
              <button
                type="button"
                onClick={() => setShowOriginal((v) => !v)}
                className="inline-flex items-center gap-1 text-xs text-mid-gray hover:text-text transition-colors cursor-pointer"
                title={t("notes.row.aiProcessed")}
              >
                <Sparkles className="w-3 h-3" aria-hidden="true" />
                {showOriginal
                  ? t("notes.row.hideOriginal")
                  : t("notes.row.showOriginal")}
              </button>
              {showOriginal && (
                <p className="mt-1 ps-3 border-s border-mid-gray/30 text-xs text-text/60 whitespace-pre-wrap break-words select-text">
                  {note.raw_text}
                </p>
              )}
            </div>
          )}

          {enabledDestinations.length > 0 && (
            <div className="flex flex-wrap items-center gap-1.5">
              {enabledDestinations.map((id) => {
                const state = note[id];
                const name = t(`notes.destinations.${id}`);
                if (state.status === "failed") {
                  return (
                    <span
                      key={id}
                      className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs bg-red-500/10 text-red-400"
                      title={t(errorKey(state.error))}
                    >
                      <CircleAlert className="w-3 h-3" aria-hidden="true" />
                      {t("notes.destinations.failed", { name })}
                    </span>
                  );
                }
                if (state.status === "pending") {
                  return (
                    <span
                      key={id}
                      className="inline-flex items-center px-2 py-0.5 rounded-full text-xs bg-mid-gray/10 text-mid-gray"
                    >
                      {t("notes.destinations.pending", { name })}
                    </span>
                  );
                }
                return (
                  <button
                    key={id}
                    type="button"
                    onClick={() => openDestination(id)}
                    className="inline-flex items-center px-2 py-0.5 rounded-full text-xs bg-mid-gray/10 text-text/70 hover:text-logo-primary hover:bg-logo-primary/10 transition-colors cursor-pointer"
                    title={t("notes.destinations.done", { name })}
                  >
                    {name}
                  </button>
                );
              })}
              {failed && (
                <button
                  type="button"
                  onClick={retry}
                  className="inline-flex items-center gap-1 px-2 py-0.5 text-xs text-mid-gray hover:text-logo-primary transition-colors cursor-pointer"
                >
                  <RefreshCw className="w-3 h-3" aria-hidden="true" />
                  {t("notes.actions.retry")}
                </button>
              )}
            </div>
          )}

          {DESTINATIONS.map((id) => {
            const state = note[id];
            return state.status === "failed" ? (
              <p key={id} className="text-xs text-text/60">
                {t(errorKey(state.error))}
              </p>
            ) : null;
          })}
        </div>
      )}
    </div>
  );
};
