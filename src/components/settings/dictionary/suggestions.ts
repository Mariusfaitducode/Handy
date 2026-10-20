import {
  commands,
  type DictionarySnapshot,
  type DictionarySuggestion,
} from "@/bindings";

/** Adds a confirmed suggestion; resolves to the new dictionary, or null. */
export const acceptSuggestion = async (
  suggestion: DictionarySuggestion,
): Promise<DictionarySnapshot | null> => {
  try {
    const result = await commands.addDictionaryEntry(
      suggestion.from,
      suggestion.kind === "replacement" ? suggestion.to : null,
    );
    if (result.status === "ok") return result.data;
    // Already there counts as done.
    if (result.error === "duplicate") return commands.getDictionary();
    return null;
  } catch (error) {
    console.error("Failed to add suggestion:", error);
    return null;
  }
};

/** Remembers a "no" so the same suggestion is not offered again. */
export const dismissSuggestion = async (suggestion: DictionarySuggestion) => {
  try {
    await commands.dismissDictionarySuggestion(suggestion.key);
  } catch (error) {
    console.error("Failed to dismiss suggestion:", error);
  }
};
