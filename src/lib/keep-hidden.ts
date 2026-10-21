// Hiding a calendar fades its events out in place, so nothing else jumps (design.md §8). Rust
// stops returning a hidden calendar's occurrences, so when the week is read again after the
// change, the ones already on screen are kept and drawn faded until the week changes.
import type { Occurrence } from './calendar.js';

/** `found` plus the occurrences of `previous` whose calendar is in `hidden`, sorted by start */
export function keepHidden(
	previous: Occurrence[],
	found: Occurrence[],
	hidden: ReadonlySet<number>
): Occurrence[] {
	const kept = previous.filter((o) => hidden.has(o.calendar_id));
	if (kept.length === 0) return found;
	// Only the order matters: an all-day date parses as midnight UTC, which is fine for that
	return [...found, ...kept].sort((a, b) => Date.parse(a.start) - Date.parse(b.start));
}
