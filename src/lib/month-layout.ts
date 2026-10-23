// What each day of the month view lists (docs/design.md §11, issue #24). Pure: no DOM, no Svelte
// state, no zone arithmetic. Built on the week view's placement, so an event lands on the same
// days in both.
import type { Occurrence } from './calendar.js';
import { placeWeek } from './week-layout.js';

/** One line in a day of the month view */
export interface MonthItem {
	occurrence: Occurrence;
	/** All-day, or timed for 24 hours or more: shown without a time, on every day it covers */
	allDay: boolean;
}

/** Per day of `days` (`YYYY-MM-DD`), the lines it lists: all-day events first, in the same order
 * on every day they share, then timed events in the order Rust sorted them. A timed event is
 * listed only on the day it starts, also when it runs past midnight. */
export function placeMonth(occurrences: Occurrence[], days: string[]): MonthItem[][] {
	const placed = placeWeek(occurrences, days);
	const lists: MonthItem[][] = days.map(() => []);
	// By row, so two events that share days keep their order on each of them
	const bars = [...placed.allDay].sort((a, b) => a.row - b.row || a.first - b.first);
	for (const bar of bars) {
		for (let day = bar.first; day <= bar.last; day++) {
			lists[day].push({ occurrence: bar.occurrence, allDay: true });
		}
	}
	placed.timed.forEach((pieces, day) => {
		for (const piece of pieces) {
			// A negative start is the part after midnight of an event that began the day before
			if (piece.start >= 0) lists[day].push({ occurrence: piece.occurrence, allDay: false });
		}
	});
	return lists;
}

/** How many of a day's `total` lines to show when `capacity` lines fit, and how many go in
 * "+N more", which takes the last line. Nothing before the cell is measured (`capacity` 0). */
export function fitDay(total: number, capacity: number): { shown: number; more: number } {
	if (capacity <= 0) return { shown: 0, more: 0 };
	if (total <= capacity) return { shown: total, more: 0 };
	const shown = capacity - 1;
	return { shown, more: total - shown };
}
