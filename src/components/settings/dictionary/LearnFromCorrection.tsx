import React, { useEffect, useState } from "react";
import { Check } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { DictionarySuggestion } from "@/bindings";
import { Button } from "../../ui/Button";
import { Term } from "./Term";
import { acceptSuggestion, dismissSuggestion } from "./suggestions";

/** How long an "Added" confirmation stays before the card closes. */
const CLOSE_DELAY_MS = 1200;

interface LearnFromCorrectionProps {
  suggestions: DictionarySuggestion[];
  onClose: () => void;
}

/**
 * Shown under a history entry the user just corrected: offers to remember
 * what changed. Nothing is learnt without a click.
 */
export const LearnFromCorrection: React.FC<LearnFromCorrectionProps> = ({
  suggestions,
  onClose,
}) => {
  const { t } = useTranslation();
  const [added, setAdded] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState<string | null>(null);

  const allAdded = suggestions.every((s) => added.has(s.key));

  useEffect(() => {
    if (!allAdded) return;
    const id = setTimeout(onClose, CLOSE_DELAY_MS);
    return () => clearTimeout(id);
  }, [allAdded, onClose]);

  const add = async (suggestion: DictionarySuggestion) => {
    setBusy(suggestion.key);
    const snapshot = await acceptSuggestion(suggestion);
    setBusy(null);
    if (snapshot) {
      setAdded((prev) => new Set(prev).add(suggestion.key));
    } else {
      toast.error(t("settings.dictionary.errors.generic"));
    }
  };

  const dismissRest = () => {
    suggestions
      .filter((s) => !added.has(s.key))
      .forEach((s) => dismissSuggestion(s));
    onClose();
  };

  return (
    <div
      className="rounded-md border border-mid-gray/20 bg-mid-gray/5 px-3 py-2 space-y-1.5"
      role="status"
    >
      <p className="text-xs font-medium text-mid-gray">
        {t("settings.history.learn.title")}
      </p>
      {suggestions.map((s) => {
        const isAdded = added.has(s.key);
        return (
          <div key={s.key} className="flex items-center gap-3">
            <div className="flex-1 min-w-0">
              <Term kind={s.kind} from={s.from} to={s.to} />
            </div>
            {isAdded ? (
              <span className="inline-flex items-center gap-1 text-xs text-text/60">
                <Check className="w-3.5 h-3.5" aria-hidden />
                {t("settings.history.learn.added")}
              </span>
            ) : (
              <Button
                variant="secondary"
                size="sm"
                disabled={busy !== null}
                onClick={() => add(s)}
              >
                {t("settings.history.learn.add")}
              </Button>
            )}
          </div>
        );
      })}
      {!allAdded && (
        <div className="flex justify-end">
          <Button variant="ghost" size="sm" onClick={dismissRest}>
            {t("settings.history.learn.dismiss")}
          </Button>
        </div>
      )}
    </div>
  );
};
