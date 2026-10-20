import { useCallback, useEffect, useState } from "react";
import {
  commands,
  events,
  type DictionarySnapshot,
  type DictionarySuggestion,
} from "@/bindings";

/**
 * Live dictionary state. Re-reads on every backend change, including a
 * background sync bringing in someone else's words.
 */
export const useDictionary = () => {
  const [snapshot, setSnapshot] = useState<DictionarySnapshot | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await commands.getDictionary());
    } catch (error) {
      console.error("Failed to load dictionary:", error);
    }
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = events.dictionaryChangedEvent.listen(() => refresh());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refresh]);

  return { snapshot, setSnapshot, refresh };
};

/** Recurring names worth adding; refreshed when the dictionary changes. */
export const useRecurringSuggestions = (dictionaryVersion: unknown) => {
  const [suggestions, setSuggestions] = useState<DictionarySuggestion[]>([]);

  useEffect(() => {
    let cancelled = false;
    commands
      .getDictionarySuggestions()
      .then((result) => {
        if (!cancelled && result.status === "ok") setSuggestions(result.data);
      })
      .catch((error) => console.error("Failed to load suggestions:", error));
    return () => {
      cancelled = true;
    };
  }, [dictionaryVersion]);

  return { suggestions, setSuggestions };
};

/** Error codes returned by the dictionary commands, as i18n keys. */
export const dictionaryErrorKey = (code: string) =>
  ["empty", "too_long", "duplicate"].includes(code)
    ? `settings.dictionary.errors.${code}`
    : "settings.dictionary.errors.generic";

/** "/Users/x/Library/Mobile Documents/com~apple~CloudDocs/Handy" → "iCloud Drive/Handy". */
export const friendlyFolderPath = (path: string) =>
  path
    .replace(
      /^\/Users\/[^/]+\/Library\/Mobile Documents\/com~apple~CloudDocs/,
      "iCloud Drive",
    )
    .replace(/^\/Users\/[^/]+\/Library\/CloudStorage\/([^/-]+)[^/]*/, "$1")
    .replace(/^\/Users\/[^/]+/, "~");
