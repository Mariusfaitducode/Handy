import React, { useRef, useState } from "react";
import { ArrowRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type DictionarySnapshot } from "@/bindings";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import { dictionaryErrorKey } from "./useDictionary";

const MAX_CHARS = 50;
/** "Andy -> Handy" typed in one field works too. */
const ARROW = /\s*(?:->|→|=>)\s*/;

interface AddEntryFormProps {
  onChange: (snapshot: DictionarySnapshot) => void;
}

export const AddEntryForm: React.FC<AddEntryFormProps> = ({ onChange }) => {
  const { t } = useTranslation();
  const [heard, setHeard] = useState("");
  const [written, setWritten] = useState("");
  const [busy, setBusy] = useState(false);
  const formRef = useRef<HTMLDivElement>(null);

  const [from, to] = (() => {
    const parts = heard.split(ARROW);
    if (!written.trim() && parts.length === 2) return [parts[0], parts[1]];
    return [heard, written];
  })();
  const isReplacement = to.trim().length > 0;
  const tooLong =
    from.trim().length > MAX_CHARS || to.trim().length > MAX_CHARS;
  const canSubmit = from.trim().length > 0 && !tooLong && !busy;

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    try {
      const result = await commands.addDictionaryEntry(
        from.trim(),
        isReplacement ? to.trim() : null,
      );
      if (result.status === "ok") {
        onChange(result.data);
        setHeard("");
        setWritten("");
        formRef.current?.querySelector("input")?.focus();
      } else {
        toast.error(
          t(dictionaryErrorKey(result.error), {
            term: isReplacement ? `${from.trim()} → ${to.trim()}` : from.trim(),
          }),
        );
      }
    } catch (error) {
      console.error("Failed to add dictionary entry:", error);
      toast.error(t("settings.dictionary.errors.generic"));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    }
  };

  return (
    <div ref={formRef} className="px-4 py-3 space-y-2">
      <div className="flex items-center gap-2">
        <Input
          type="text"
          className="flex-1 min-w-0"
          variant="compact"
          value={heard}
          maxLength={MAX_CHARS * 2 + 4}
          onChange={(e) => setHeard(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={
            written
              ? t("settings.dictionary.add.heardPlaceholderReplacement")
              : t("settings.dictionary.add.heardPlaceholder")
          }
          aria-label={t("settings.dictionary.add.heardPlaceholder")}
        />
        <ArrowRight
          className={`w-4 h-4 shrink-0 transition-colors ${
            isReplacement ? "text-text/70" : "text-mid-gray/50"
          }`}
          aria-hidden
        />
        <Input
          type="text"
          className="flex-1 min-w-0"
          variant="compact"
          value={written}
          maxLength={MAX_CHARS}
          onChange={(e) => setWritten(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={t("settings.dictionary.add.writtenPlaceholder")}
          aria-label={t("settings.dictionary.add.writtenPlaceholder")}
        />
        <Button
          onClick={submit}
          disabled={!canSubmit}
          variant="primary"
          size="md"
        >
          {t("settings.dictionary.add.button")}
        </Button>
      </div>
      <p className={`text-xs ${tooLong ? "text-red-400" : "text-mid-gray"}`}>
        {tooLong
          ? t("settings.dictionary.errors.too_long")
          : t("settings.dictionary.add.hint")}
      </p>
    </div>
  );
};
