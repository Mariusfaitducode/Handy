import React from "react";
import { ArrowRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { EntryKind } from "@/bindings";

interface TermProps {
  kind: EntryKind;
  from: string;
  to?: string;
}

/** A word, or "heard → written" for a replacement. */
export const Term: React.FC<TermProps> = ({ kind, from, to }) => {
  const { t } = useTranslation();

  if (kind === "word" || !to) {
    return <span className="text-sm font-medium break-words">{from}</span>;
  }

  return (
    <span className="inline-flex items-center gap-1.5 min-w-0 flex-wrap text-sm">
      <span className="text-text/50 line-through decoration-text/20 break-words">
        {from}
      </span>
      <ArrowRight
        className="w-3.5 h-3.5 shrink-0 text-mid-gray"
        aria-label={t("settings.dictionary.add.becomes")}
      />
      <span className="font-medium break-words">{to}</span>
    </span>
  );
};
