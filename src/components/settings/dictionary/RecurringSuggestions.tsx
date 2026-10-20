import React from "react";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { DictionarySnapshot, DictionarySuggestion } from "@/bindings";
import { Button } from "../../ui/Button";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { Term } from "./Term";
import { acceptSuggestion, dismissSuggestion } from "./suggestions";

interface RecurringSuggestionsProps {
  suggestions: DictionarySuggestion[];
  onSuggestionsChange: (suggestions: DictionarySuggestion[]) => void;
  onChange: (snapshot: DictionarySnapshot) => void;
}

/** Names that keep coming back in dictations. Hidden when there are none. */
export const RecurringSuggestions: React.FC<RecurringSuggestionsProps> = ({
  suggestions,
  onSuggestionsChange,
  onChange,
}) => {
  const { t } = useTranslation();

  if (suggestions.length === 0) return null;

  const drop = (key: string) =>
    onSuggestionsChange(suggestions.filter((s) => s.key !== key));

  const add = async (suggestion: DictionarySuggestion) => {
    drop(suggestion.key);
    const snapshot = await acceptSuggestion(suggestion);
    if (snapshot) {
      onChange(snapshot);
      toast.success(
        t("settings.dictionary.add.added", { term: suggestion.from }),
      );
    } else {
      toast.error(t("settings.dictionary.errors.generic"));
    }
  };

  const dismiss = (suggestion: DictionarySuggestion) => {
    drop(suggestion.key);
    dismissSuggestion(suggestion);
  };

  return (
    <SettingsGroup
      title={t("settings.dictionary.suggestions.title")}
      description={t("settings.dictionary.suggestions.description")}
    >
      {suggestions.map((s) => (
        <div key={s.key} className="flex items-center gap-3 px-4 py-2">
          <div className="flex-1 min-w-0 flex items-baseline gap-2">
            <Term kind={s.kind} from={s.from} to={s.to} />
            {s.occurrences !== null && (
              <span className="text-xs text-mid-gray">
                {t("settings.dictionary.suggestions.seen", {
                  count: s.occurrences,
                })}
              </span>
            )}
          </div>
          <Button variant="secondary" size="sm" onClick={() => add(s)}>
            {t("settings.dictionary.suggestions.add")}
          </Button>
          <button
            onClick={() => dismiss(s)}
            className="p-1 rounded-md text-text/40 hover:text-text/80 focus:outline-none focus-visible:ring-1 focus-visible:ring-logo-primary cursor-pointer"
            aria-label={t("settings.dictionary.suggestions.dismissLabel", {
              term: s.from,
            })}
            title={t("settings.dictionary.suggestions.dismiss")}
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      ))}
    </SettingsGroup>
  );
};
