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

/** The placements of one day's events, in drawing order: an event drawn later sits on top. */
export function layoutDay(
	events: TimedSpan[],
	options: LayoutOptions = DEFAULT_LAYOUT
): Placement[] {
	void events;
	void options;
	throw new Error('not implemented');
}
