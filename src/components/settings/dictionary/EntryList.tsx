import React, { useMemo, useState } from "react";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  commands,
  type DictionaryEntry,
  type DictionarySnapshot,
} from "@/bindings";
import { formatDate } from "@/utils/dateFormat";
import { Input } from "../../ui/Input";
import { Term } from "./Term";

/** Below this the whole list fits at a glance; no search needed. */
const FILTER_THRESHOLD = 8;

const fold = (s: string) =>
  s
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .toLowerCase();

interface EntryListProps {
  snapshot: DictionarySnapshot;
  onChange: (snapshot: DictionarySnapshot) => void;
}

export const EntryList: React.FC<EntryListProps> = ({ snapshot, onChange }) => {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const { entries } = snapshot;

  const visible = useMemo(() => {
    const q = fold(query.trim());
    if (!q) return entries;
    return entries.filter(
      (e) => fold(e.from).includes(q) || fold(e.to ?? "").includes(q),
    );
  }, [entries, query]);

  const remove = async (entry: DictionaryEntry) => {
    try {
      const result = await commands.removeDictionaryEntry(entry.id);
      if (result.status !== "ok") {
        toast.error(t("settings.dictionary.errors.generic"));
        return;
      }
      onChange(result.data);
      const term =
        entry.kind === "replacement"
          ? `${entry.from} → ${entry.to}`
          : entry.from;
      toast(t("settings.dictionary.entry.removed", { term }), {
        action: {
          label: t("settings.dictionary.entry.undo"),
          onClick: async () => {
            const restored = await commands.addDictionaryEntry(
              entry.from,
              entry.kind === "replacement" ? (entry.to ?? null) : null,
            );
            if (restored.status === "ok") onChange(restored.data);
          },
        },
      });
    } catch (error) {
      console.error("Failed to remove dictionary entry:", error);
      toast.error(t("settings.dictionary.errors.generic"));
    }
  };

  if (entries.length === 0) {
    return (
      <div className="px-4 py-6 text-center space-y-1">
        <p className="text-sm font-medium text-text/80">
          {t("settings.dictionary.empty.title")}
        </p>
        <p className="text-xs text-mid-gray max-w-md mx-auto leading-relaxed">
          {t("settings.dictionary.empty.body")}
        </p>
      </div>
    );
  }

  return (
    <div>
      {entries.length > FILTER_THRESHOLD && (
        <div className="px-4 pt-3 pb-1">
          <Input
            type="search"
            variant="compact"
            className="w-full"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("settings.dictionary.filterPlaceholder", {
              count: entries.length,
            })}
          />
        </div>
      )}
      {visible.length === 0 ? (
        <p className="px-4 py-3 text-sm text-mid-gray">
          {t("settings.dictionary.noMatch", { query: query.trim() })}
        </p>
      ) : (
        <ul className="py-1 max-h-[26rem] overflow-y-auto">
          {visible.map((entry) => (
            <EntryRow
              key={entry.id}
              entry={entry}
              me={snapshot.author}
              onRemove={() => remove(entry)}
            />
          ))}
        </ul>
      )}
    </div>
  );
};

const EntryRow: React.FC<{
  entry: DictionaryEntry;
  me: string;
  onRemove: () => void;
}> = ({ entry, me, onRemove }) => {
  const { t, i18n } = useTranslation();
  const term =
    entry.kind === "replacement" ? `${entry.from} → ${entry.to}` : entry.from;
  const fromSomeoneElse =
    !!entry.added_by && entry.added_by.toLowerCase() !== me.toLowerCase();
  const date = entry.created_at
    ? formatDate(String(Math.floor(entry.created_at / 1000)), i18n.language)
    : "";
  const detail = fromSomeoneElse
    ? t("settings.dictionary.entry.addedBy", { name: entry.added_by, date })
    : t("settings.dictionary.entry.addedOn", { date });

  return (
    <li
      className="group flex items-center gap-3 px-4 py-1.5 hover:bg-mid-gray/5"
      title={date ? detail : undefined}
    >
      <div className="flex-1 min-w-0">
        <Term kind={entry.kind} from={entry.from} to={entry.to} />
      </div>
      {fromSomeoneElse && (
        <span className="text-xs text-mid-gray shrink-0">{entry.added_by}</span>
      )}
      <button
        onClick={onRemove}
        className="p-1 rounded-md text-text/40 hover:text-red-400 opacity-0 group-hover:opacity-100 focus:opacity-100 focus:outline-none focus-visible:ring-1 focus-visible:ring-logo-primary transition-opacity cursor-pointer"
        aria-label={t("settings.dictionary.entry.remove", { term })}
        title={t("settings.dictionary.entry.remove", { term })}
      >
        <X className="w-3.5 h-3.5" />
      </button>
    </li>
  );
};
