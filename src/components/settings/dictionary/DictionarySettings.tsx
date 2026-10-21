import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { AddEntryForm } from "./AddEntryForm";
import { EntryList } from "./EntryList";
import { RecurringSuggestions } from "./RecurringSuggestions";
import { SharingSettings } from "./SharingSettings";
import { useDictionary, useRecurringSuggestions } from "./useDictionary";

export const DictionarySettings: React.FC = () => {
  const { t } = useTranslation();
  const { snapshot, setSnapshot } = useDictionary();
  const { suggestions, setSuggestions } = useRecurringSuggestions(
    snapshot?.entries.length,
  );

  if (!snapshot) {
    return <div className="max-w-3xl w-full mx-auto" />;
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup
        title={t("settings.dictionary.title")}
        description={t("settings.dictionary.description")}
      >
        <AddEntryForm onChange={setSnapshot} />
        <EntryList snapshot={snapshot} onChange={setSnapshot} />
      </SettingsGroup>

      <RecurringSuggestions
        suggestions={suggestions}
        onSuggestionsChange={setSuggestions}
        onChange={setSnapshot}
      />

      <SharingSettings snapshot={snapshot} onChange={setSnapshot} />
    </div>
  );
};
