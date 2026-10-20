import assert from "node:assert/strict";
import type { VoiceNote } from "@/bindings";
import {
  clipboardText,
  dayLabel,
  errorKey,
  groupByDay,
  hasFailedDestination,
  matchesQuery,
} from "./noteUtils";

const note = (overrides: Partial<VoiceNote>): VoiceNote => ({
  id: 1,
  created_at: 0,
  title: "Idée pour l'été",
  body: "Mettre la démo vidéo en premier.",
  raw_text: "euh mettre la démo vidéo en premier",
  ai_processed: true,
  markdown: { status: "off" },
  notion: { status: "off" },
  apple_notes: { status: "off" },
  ...overrides,
});

// Search ignores case and accents, and needs every term.
assert.equal(matchesQuery(note({}), "ete"), true);
assert.equal(matchesQuery(note({}), "DEMO video"), true);
assert.equal(matchesQuery(note({}), "demo podcast"), false);
assert.equal(matchesQuery(note({}), "   "), true);

// Grouping keeps order and splits on local calendar days.
const at = (y: number, m: number, d: number, h: number) =>
  new Date(y, m, d, h).getTime() / 1000;
const groups = groupByDay([
  note({ id: 3, created_at: at(2026, 9, 21, 18) }),
  note({ id: 2, created_at: at(2026, 9, 21, 9) }),
  note({ id: 1, created_at: at(2026, 9, 20, 23) }),
]);
assert.deepEqual(
  groups.map((g) => g.notes.map((n) => n.id)),
  [[3, 2], [1]],
);

const now = new Date(2026, 9, 21, 12);
assert.deepEqual(dayLabel(new Date(2026, 9, 21), now), { kind: "today" });
assert.deepEqual(dayLabel(new Date(2026, 9, 20), now), { kind: "yesterday" });
assert.deepEqual(dayLabel(new Date(2026, 0, 2), now), {
  kind: "date",
  sameYear: true,
});
assert.deepEqual(dayLabel(new Date(2025, 11, 31), now), {
  kind: "date",
  sameYear: false,
});

// Error codes map to translations; details and unknown codes don't leak.
assert.equal(
  errorKey("notion_network: timeout"),
  "notes.errors.notion_network",
);
assert.equal(errorKey("io: disk full"), "notes.errors.io");
assert.equal(errorKey("weird"), "notes.errors.generic");

assert.equal(hasFailedDestination(note({})), false);
assert.equal(
  hasFailedDestination(note({ notion: { status: "failed", error: "x" } })),
  true,
);

assert.equal(clipboardText(note({ title: "T", body: "B" })), "T\n\nB");
assert.equal(clipboardText(note({ title: "", body: "B" })), "B");

console.log("noteUtils: all assertions passed");
