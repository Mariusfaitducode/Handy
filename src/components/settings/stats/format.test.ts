import assert from "node:assert/strict";
import {
  barHeight,
  durationParts,
  languageName,
  parseLocalDay,
  percentage,
} from "./format";

assert.deepEqual(durationParts(0), { kind: "lessThanMinute" });
assert.deepEqual(durationParts(59), { kind: "lessThanMinute" });
assert.deepEqual(durationParts(60), { kind: "minutes", minutes: 1 });
assert.deepEqual(durationParts(12 * 60 + 20), { kind: "minutes", minutes: 12 });
assert.deepEqual(durationParts(3600), { kind: "hours", hours: 1 });
assert.deepEqual(durationParts(4 * 3600 + 12 * 60), {
  kind: "hoursMinutes",
  hours: 4,
  minutes: 12,
});
assert.deepEqual(durationParts(12 * 3600 + 40 * 60), {
  kind: "hours",
  hours: 13,
});
assert.deepEqual(durationParts(-5), { kind: "lessThanMinute" });

const day = parseLocalDay("2026-10-21");
assert.ok(day);
assert.equal(day.getFullYear(), 2026);
assert.equal(day.getMonth(), 9);
assert.equal(day.getDate(), 21);
assert.equal(parseLocalDay("not a day"), null);

assert.equal(percentage(0, 100), 0);
assert.equal(percentage(1, 1000), 1);
assert.equal(percentage(42, 100), 42);
assert.equal(percentage(5, 0), 0);

assert.equal(barHeight(0, 100), 0);
assert.equal(barHeight(1, 1000), 6);
assert.equal(barHeight(100, 100), 100);

assert.equal(languageName("fr", "fr"), "Français");
assert.equal(languageName("en", "en"), "English");

console.log("stats format: all assertions passed");
