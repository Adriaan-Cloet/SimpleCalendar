// Where the occurrences from `list_occurrences` go in the week view (docs/design.md §11). Pure:
// no DOM, no Svelte state, and no zone arithmetic. Rust already returned every time as a
// wall-clock time in the display zone, so a timed value's date and time are read as written.
import type { Occurrence } from './calendar.js';

/** A timed occurrence on one day, in minutes since that day's midnight. On the second day of an
 * occurrence that runs past midnight `start` is negative; on the first, `end` is past 1440. The
 * overlap layout clips both. */
export interface TimedPiece {
	occurrence: Occurrence;
	start: number;
	end: number;
}

/** An occurrence in the all-day row, over days `first` to `last` of the week (0 is Monday) */
export interface AllDayBar {
	occurrence: Occurrence;
	first: number;
	last: number;
	row: number;
}

export interface WeekLayout {
	/** Per day of the week, its timed pieces in the order Rust sorted them */
	timed: TimedPiece[][];
	/** In drawing order */
	allDay: AllDayBar[];
	/** Rows the all-day bars need */
	rows: number;
}

const DAY = 24 * 60;

/** Days since 1970 for a `YYYY-MM-DD` date: plain calendar counting, with no zone involved */
function dayNumber(date: string): number {
	const [y, m, d] = date.slice(0, 10).split('-').map(Number);
	return Date.UTC(y, m - 1, d) / (DAY * 60_000);
}

/** The day and minute of a timed value, as written */
function wallClock(value: string): { day: number; minutes: number } {
	return {
		day: dayNumber(value),
		minutes: Number(value.slice(11, 13)) * 60 + Number(value.slice(14, 16))
	};
}

/** Places a week's occurrences. `days` are the week's seven dates, `YYYY-MM-DD`.
 *
 * All-day occurrences go to the all-day row, and so do timed ones of 24 hours or more, which
 * would otherwise fill whole columns. A shorter timed occurrence is drawn on every day it
 * touches; one that ends exactly at midnight is not drawn on the next day. */
export function placeWeek(occurrences: Occurrence[], days: string[]): WeekLayout {
	const firstDay = dayNumber(days[0]);
	const lastDay = firstDay + days.length - 1;
	const timed: TimedPiece[][] = days.map(() => []);
	const spans: { occurrence: Occurrence; first: number; last: number }[] = [];

	for (const occurrence of occurrences) {
		if (!occurrence.start.includes('T')) {
			// The end date is exclusive
			spans.push({
				occurrence,
				first: dayNumber(occurrence.start),
				last: dayNumber(occurrence.end) - 1
			});
			continue;
		}
		const start = wallClock(occurrence.start);
		const end = wallClock(occurrence.end);
		const length = (end.day - start.day) * DAY + end.minutes - start.minutes;
		// A piece that would start at the end's midnight is empty
		const last = end.minutes === 0 && end.day > start.day ? end.day - 1 : end.day;
		if (length >= DAY) {
			spans.push({ occurrence, first: start.day, last });
			continue;
		}
		for (let day = Math.max(start.day, firstDay); day <= Math.min(last, lastDay); day++) {
			const offset = (day - start.day) * DAY;
			timed[day - firstDay].push({
				occurrence,
				start: start.minutes - offset,
				end: start.minutes + length - offset
			});
		}
	}

	const shown = spans
		.filter((s) => s.last >= firstDay && s.first <= lastDay)
		.map((s, i) => ({ ...s, i }))
		.sort(
			(a, b) =>
				a.first - b.first ||
				b.last - a.last ||
				a.occurrence.start.localeCompare(b.occurrence.start) ||
				a.i - b.i
		);

	// Each bar takes the first row that is free on all its days. `rowEnds[r]` is the last day
	// taken in row r so far; bars come in order of their first day, so that is enough.
	const rowEnds: number[] = [];
	const allDay = shown.map(({ occurrence, first, last }) => {
		const from = Math.max(first, firstDay) - firstDay;
		const to = Math.min(last, lastDay) - firstDay;
		let row = rowEnds.findIndex((end) => end < from);
		if (row < 0) row = rowEnds.length;
		rowEnds[row] = to;
		return { occurrence, first: from, last: to, row };
	});

	return { timed, allDay, rows: rowEnds.length };
}
