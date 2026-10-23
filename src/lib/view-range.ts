// Which days the day and week views show around the selected date, how the toolbar's arrows
// move that date, and the toolbar title. Local calendar dates only (architecture §10).
import { addDays, startOfWeek } from './week-dates.js';

export type View = 'day' | 'week';

/** The days on screen: `days` local dates from `start` */
export interface ViewRange {
	start: Date;
	days: number;
}

/** The days `view` shows for the selected `date`. Both views keep the same date, so switching
 * between them shows the day within its week and back. */
export function viewRange(view: View, date: Date): ViewRange {
	if (view === 'week') return { start: startOfWeek(date), days: 7 };
	return { start: new Date(date.getFullYear(), date.getMonth(), date.getDate()), days: 1 };
}

/** The selected date one step back (`-1`) or on (`1`): a day, or a week with the same weekday */
export function step(view: View, date: Date, direction: 1 | -1): Date {
	return addDays(date, direction * (view === 'week' ? 7 : 1));
}

/** Whether `moment` falls on one of the days of `range` */
export function inRange(range: ViewRange, moment: Date): boolean {
	return moment >= range.start && moment < addDays(range.start, range.days);
}

/** "October 2026" or "Oct – Nov 2026" for a week; "Friday 23 October 2026" for a day */
export function viewTitle(view: View, date: Date, locale: string): string {
	if (view === 'day') {
		return new Intl.DateTimeFormat(locale, { dateStyle: 'full' }).format(date);
	}
	const { start, days } = viewRange(view, date);
	return new Intl.DateTimeFormat(locale, { month: 'long', year: 'numeric' }).formatRange(
		start,
		addDays(start, days - 1)
	);
}
