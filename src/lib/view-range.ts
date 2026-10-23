// Which days the day, week and month views show around the selected date, how the toolbar's
// arrows move that date, and the toolbar title. Local calendar dates only (architecture §10).
import { addDays, monthWeeks, startOfWeek } from './week-dates.js';

export type View = 'day' | 'week' | 'month';

/** The days on screen: `days` local dates from `start` */
export interface ViewRange {
	start: Date;
	days: number;
}

/** The days `view` shows for the selected `date`: for a month, the whole weeks that cover it.
 * Every view keeps the same date, so switching shows the day within its week or month and back. */
export function viewRange(view: View, date: Date): ViewRange {
	if (view === 'week') return { start: startOfWeek(date), days: 7 };
	if (view === 'month') {
		const weeks = monthWeeks(date.getFullYear(), date.getMonth());
		return { start: weeks[0][0], days: weeks.length * 7 };
	}
	return { start: new Date(date.getFullYear(), date.getMonth(), date.getDate()), days: 1 };
}

/** The days that belong to `view`: for a month only its own days, not those of the months
 * around it that fill its first and last week */
export function ownRange(view: View, date: Date): ViewRange {
	if (view !== 'month') return viewRange(view, date);
	return {
		start: new Date(date.getFullYear(), date.getMonth(), 1),
		days: daysInMonth(date.getFullYear(), date.getMonth())
	};
}

/** The selected date one step back (`-1`) or on (`1`): a day, a week with the same weekday, or a
 * month with the same day, or that month's last day if it is shorter */
export function step(view: View, date: Date, direction: 1 | -1): Date {
	if (view === 'month') {
		const year = date.getFullYear();
		const month = date.getMonth() + direction;
		return new Date(year, month, Math.min(date.getDate(), daysInMonth(year, month)));
	}
	return addDays(date, direction * (view === 'week' ? 7 : 1));
}

/** Days in month `month` (0-based, may run over into the next year) of `year` */
function daysInMonth(year: number, month: number): number {
	// Day 0 of the next month is the last day of this one
	return new Date(year, month + 1, 0).getDate();
}

/** Whether `moment` falls on one of the days of `range` */
export function inRange(range: ViewRange, moment: Date): boolean {
	return moment >= range.start && moment < addDays(range.start, range.days);
}

/** "October 2026" or "Oct – Nov 2026" for a week; "Friday 23 October 2026" for a day; the
 * month itself for a month */
export function viewTitle(view: View, date: Date, locale: string): string {
	if (view === 'day') {
		return new Intl.DateTimeFormat(locale, { dateStyle: 'full' }).format(date);
	}
	if (view === 'month') {
		return new Intl.DateTimeFormat(locale, { month: 'long', year: 'numeric' }).format(date);
	}
	const { start, days } = viewRange(view, date);
	return new Intl.DateTimeFormat(locale, { month: 'long', year: 'numeric' }).formatRange(
		start,
		addDays(start, days - 1)
	);
}
