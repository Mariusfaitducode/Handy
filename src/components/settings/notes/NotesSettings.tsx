import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Search, Settings2, X } from "lucide-react";
import { commands, events, type VoiceNote } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { useOsType } from "@/hooks/useOsType";
import { formatKeyCombination } from "@/lib/utils/keyboard";
import { Button } from "../../ui/Button";
import { NoteRow } from "./NoteRow";
import { NotesSettingsPanel } from "./NotesSettingsPanel";
import { dayLabel, groupByDay, matchesQuery } from "./noteUtils";

const UNDO_WINDOW_MS = 5000;

export const NotesSettings: React.FC = () => {
  const { t, i18n } = useTranslation();
  const osType = useOsType();
  const { settings, refreshSettings } = useSettings();
  const [view, setView] = useState<"list" | "settings">("list");
  const [notes, setNotes] = useState<VoiceNote[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [query, setQuery] = useState("");
  const [expandedId, setExpandedId] = useState<number | null>(null);
  // Notes hidden while their "Undo" toast is open; deleted for real after.
  const [pendingDeletes, setPendingDeletes] = useState<Set<number>>(new Set());
  const searchRef = useRef<HTMLInputElement>(null);

  const voiceNotes = settings?.voice_notes;
  const enabled = voiceNotes?.enabled ?? true;
  const shortcut = settings?.bindings?.voice_note?.current_binding;

  const load = useCallback(async () => {
    try {
      const result = await commands.getVoiceNotes();
      if (result.status === "ok") {
        setNotes(result.data);
        setLoadFailed(false);
      } else {
        setLoadFailed(true);
      }
    } catch (error) {
      console.error("Failed to load voice notes:", error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
    const unlisten = events.voiceNotesChanged.listen(() => load());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [load]);

  const visibleNotes = useMemo(
    () =>
      notes.filter((n) => !pendingDeletes.has(n.id) && matchesQuery(n, query)),
    [notes, pendingDeletes, query],
  );
  const groups = useMemo(() => groupByDay(visibleNotes), [visibleNotes]);
  const totalCount = notes.filter((n) => !pendingDeletes.has(n.id)).length;

  const formatDay = (day: Date): string => {
    const label = dayLabel(day);
    if (label.kind === "today") return t("notes.today");
    if (label.kind === "yesterday") return t("notes.yesterday");
    return new Intl.DateTimeFormat(i18n.language, {
      weekday: "long",
      day: "numeric",
      month: "long",
      ...(label.sameYear ? {} : { year: "numeric" }),
    }).format(day);
  };

  const requestDelete = (note: VoiceNote) => {
    setPendingDeletes((prev) => new Set(prev).add(note.id));
    if (expandedId === note.id) setExpandedId(null);

    let settled = false;
    const restore = () => {
      if (settled) return;
      settled = true;
      setPendingDeletes((prev) => {
        const next = new Set(prev);
        next.delete(note.id);
        return next;
      });
    };
    const commit = async () => {
      if (settled) return;
      settled = true;
      try {
        const result = await commands.deleteVoiceNote(note.id);
        if (result.status !== "ok") throw new Error(result.error);
        if (result.data === "left_untouched") {
          toast.info(t("notes.toasts.deleteLeftUntouched"));
        }
      } catch (error) {
        console.error("Failed to delete voice note:", error);
        toast.error(t("notes.toasts.deleteError"));
      } finally {
        setPendingDeletes((prev) => {
          const next = new Set(prev);
          next.delete(note.id);
          return next;
        });
        load();
      }
    };

    toast(t("notes.toasts.deleted"), {
      description: note.title || undefined,
      duration: UNDO_WINDOW_MS,
      action: { label: t("notes.toasts.undo"), onClick: restore },
      onAutoClose: commit,
      onDismiss: commit,
    });
  };

  const enableVoiceNotes = async () => {
    if (!voiceNotes) return;
    const result = await commands.updateVoiceNotesSettings({
      ...voiceNotes,
      enabled: true,
    });
    if (result.status !== "ok") {
      toast.error(t("notes.toasts.saveError"));
    }
    await refreshSettings();
  };

  if (view === "settings") {
    return <NotesSettingsPanel onBack={() => setView("list")} />;
  }

  const shortcutLabel = shortcut ? formatKeyCombination(shortcut, osType) : "";

  let content: React.ReactNode;
  if (loading) {
    content = (
      <p className="px-4 py-6 text-center text-sm text-text/60">
        {t("notes.loading")}
      </p>
    );
  } else if (loadFailed) {
    content = (
      <p className="px-4 py-6 text-center text-sm text-text/60">
        {t("notes.loadError")}
      </p>
    );
  } else if (totalCount === 0) {
    content = enabled ? (
      <div className="px-6 py-10 text-center space-y-2">
        <p className="text-sm font-medium">{t("notes.empty.title")}</p>
        <p className="text-sm text-text/60 max-w-sm mx-auto leading-relaxed">
          {t("notes.empty.description", { shortcut: shortcutLabel })}
        </p>
      </div>
    ) : (
      <div className="px-6 py-10 text-center space-y-3">
        <p className="text-sm font-medium">{t("notes.disabled.title")}</p>
        <p className="text-sm text-text/60 max-w-sm mx-auto leading-relaxed">
          {t("notes.disabled.description")}
        </p>
        <Button variant="primary-soft" size="sm" onClick={enableVoiceNotes}>
          {t("notes.disabled.enable")}
        </Button>
      </div>
    );
  } else if (visibleNotes.length === 0) {
    content = (
      <p className="px-4 py-6 text-center text-sm text-text/60">
        {t("notes.noResults", { query: query.trim() })}
      </p>
    );
  } else {
    content = groups.map((group) => (
      <section key={group.key}>
        <h3 className="px-4 pt-3 pb-1 text-xs font-medium text-mid-gray first-letter:uppercase">
          {formatDay(group.day)}
        </h3>
        <div className="divide-y divide-mid-gray/10">
          {group.notes.map((note) => (
            <NoteRow
              key={note.id}
              note={note}
              expanded={expandedId === note.id}
              onToggle={() =>
                setExpandedId((id) => (id === note.id ? null : note.id))
              }
              onDelete={() => requestDelete(note)}
            />
          ))}
        </div>
      </section>
    ));
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-2">
      <div className="px-4 flex items-center justify-between gap-3 min-h-8">
        <div className="flex items-baseline gap-2">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("notes.title")}
          </h2>
          {totalCount > 0 && (
            <span className="text-xs text-mid-gray/80 tabular-nums">
              {t("notes.count", { count: totalCount })}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1">
          {totalCount > 0 && (
            <div className="relative">
              <Search
                className="absolute start-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-mid-gray pointer-events-none"
                aria-hidden="true"
              />
              <input
                ref={searchRef}
                type="search"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setQuery("");
                }}
                placeholder={t("notes.searchPlaceholder")}
                aria-label={t("notes.searchPlaceholder")}
                className="w-56 ps-7 pe-7 py-1 text-sm bg-mid-gray/10 border border-mid-gray/20 rounded-md focus:outline-none focus:border-logo-primary transition-colors [&::-webkit-search-cancel-button]:hidden"
              />
              {query && (
                <button
                  type="button"
                  onClick={() => {
                    setQuery("");
                    searchRef.current?.focus();
                  }}
                  className="absolute end-1.5 top-1/2 -translate-y-1/2 p-0.5 text-mid-gray hover:text-text cursor-pointer"
                  aria-label={t("notes.clearSearch")}
                  title={t("notes.clearSearch")}
                >
                  <X className="w-3.5 h-3.5" />
                </button>
              )}
            </div>
          )}
          <button
            type="button"
            onClick={() => setView("settings")}
            className="p-1.5 rounded-md text-text/50 hover:text-logo-primary hover:bg-mid-gray/10 transition-colors cursor-pointer"
            title={t("notes.settingsButton")}
            aria-label={t("notes.settingsButton")}
          >
            <Settings2 className="w-4 h-4" />
          </button>
        </div>
      </div>
      <div className="bg-background border border-mid-gray/20 rounded-lg overflow-hidden pb-1">
        {content}
      </div>
    </div>
  );
};
