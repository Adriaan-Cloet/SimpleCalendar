// Where each timed event of one day goes in the week and day views (docs/architecture.md §10,
// docs/design.md §11). Pure: plain numbers in, plain numbers out, no DOM, no Svelte state.

/** One timed occurrence, in minutes since the start of the day. It may start before 0 or end
 * after 1440 (an event that runs past midnight); the end is exclusive. */
export interface TimedSpan {
	id: string;
	start: number;
	end: number;
}

export interface LayoutOptions {
	/** The shortest an event is drawn, in minutes. Overlaps are judged on the drawn size. */
	minDuration: number;
	/** How much later an overlapping event must start to be stacked on top of an earlier one
	 * instead of beside it: the height of the earlier event's title line, at 1px per minute. */
	titleClearance: number;
	/** How many events may stack in one column before the next one gets a column of its own. */
	maxIndent: number;
}

/** 15 minutes: the height of a one-line event. 20 minutes: a title line (18px) plus room. */
export const DEFAULT_LAYOUT: LayoutOptions = { minDuration: 15, titleClearance: 20, maxIndent: 3 };

export interface Placement {
	id: string;
	/** Drawn extent in minutes since the start of the day, clipped to the day */
	top: number;
	bottom: number;
	/** Column of the event's overlap group, from the left */
	column: number;
	/** Columns in the event's overlap group */
	columns: number;
	/** Columns the event covers, from `column` to the right (at least 1) */
	span: number;
	/** Stacking depth inside its column: 0 is flush left, each level is indented and drawn on
	 * top of the one below */
	indent: number;
	/** It overlaps another event; the view separates it with a channel */
	touching: boolean;
	/** It starts later than an event it overlaps, so it lies over it; the view draws it one
	 * tint stronger */
	raised: boolean;
}

const DAY = 24 * 60;

/** The placements of one day's events, in drawing order: an event drawn later sits on top.
 *
 * Events are taken by start time, longer first. Each one goes, in this order of preference:
 * 1. into the leftmost column that is free for its whole drawn length;
 * 2. on top of a column's stack, indented, if it starts at least `titleClearance` after every
 *    event there that it overlaps, so their titles stay visible, and the stack is not full;
 * 3. into a new column.
 * Columns are counted per group of events that overlap each other, directly or through others.
 * Finally every event widens to the right over the columns that are free for its whole length.
 */
export function layoutDay(
	events: TimedSpan[],
	options: LayoutOptions = DEFAULT_LAYOUT
): Placement[] {
	const placements = events.flatMap((event) => {
		const pointInDay = event.start === event.end && event.start >= 0 && event.start < DAY;
		if (!pointInDay && (event.end <= 0 || event.start >= DAY)) return [];
		const top = Math.max(event.start, 0);
		const bottom = Math.min(DAY, Math.max(Math.min(event.end, DAY), top + options.minDuration));
		return [
			{
				id: event.id,
				top,
				bottom,
				column: 0,
				columns: 1,
				span: 1,
				indent: 0,
				touching: false,
				raised: false
			}
		];
	});
	// The id breaks ties, so the result does not depend on the input order
	placements.sort(
		(a, b) => a.top - b.top || b.bottom - a.bottom || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
	);

	let columns: Placement[][] = [];
	let groupBottom = -Infinity;
	for (const placement of placements) {
		// Nothing placed so far reaches this event: the previous group is complete
		if (placement.top >= groupBottom) {
			finishGroup(columns);
			columns = [];
		}
		place(placement, columns, options);
		groupBottom = Math.max(groupBottom, placement.bottom);
	}
	finishGroup(columns);
	return placements;
}

function overlaps(a: Placement, b: Placement): boolean {
	return a.top < b.bottom && b.top < a.bottom;
}

/** Puts `placement` into a column of its group, as described at `layoutDay`. */
function place(placement: Placement, columns: Placement[][], options: LayoutOptions) {
	const overlapping = columns.map((column) => column.filter((p) => overlaps(p, placement)));

	let column = overlapping.findIndex((others) => others.length === 0);
	let indent = 0;
	if (column < 0) {
		column = overlapping.findIndex(
			(others) =>
				others.every((p) => placement.top - p.top >= options.titleClearance) &&
				Math.max(...others.map((p) => p.indent)) < options.maxIndent
		);
		if (column >= 0) indent = Math.max(...overlapping[column].map((p) => p.indent)) + 1;
	}
	if (column < 0) {
		column = columns.length;
		columns.push([]);
	}
	placement.column = column;
	placement.indent = indent;
	columns[column].push(placement);
}

/** Sets the column count of a finished group, widens its events into free columns and marks
 * which events touch or lie over another. */
function finishGroup(columns: Placement[][]) {
	const group = columns.flat();
	for (const placement of group) {
		const others = group.filter((p) => p !== placement && overlaps(p, placement));
		placement.touching = others.length > 0;
		placement.raised = others.some((p) => p.top < placement.top);
	}
	for (const column of columns) {
		for (const placement of column) {
			placement.columns = columns.length;
			placement.span = 1;
			for (let right = placement.column + 1; right < columns.length; right++) {
				if (columns[right].some((p) => overlaps(p, placement))) break;
				placement.span++;
			}
		}
	}
}
