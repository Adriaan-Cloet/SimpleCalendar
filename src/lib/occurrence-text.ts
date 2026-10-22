// How an occurrence's times read in the views. Rust sends wall-clock times in the display zone,
// which is this computer's zone, so a local `Date` built from the written date and time shows
// them unchanged. Nothing here converts between zones.
import type { Occurrence, Repeat } from './calendar.js';
import { displayLocale } from './display-format.js';

/** A `YYYY-MM-DD` date or a timed value from Rust as a local `Date`, at the time written */
export function wallClock(value: string): Date {
	const [y, m, d] = value.slice(0, 10).split('-').map(Number);
	const timed = value.includes('T');
	const h = timed ? Number(value.slice(11, 13)) : 0;
	const min = timed ? Number(value.slice(14, 16)) : 0;
	return new Date(y, m - 1, d, h, min);
}

/** The popover's date and time lines, e.g. `['Tuesday 20 October', '09:00–10:30']` */
export function describeWhen(o: Occurrence, locale = displayLocale): string[] {
	const date = new Intl.DateTimeFormat(locale, { weekday: 'long', day: 'numeric', month: 'long' });
	const start = wallClock(o.start);
	const end = wallClock(o.end);
	if (!o.start.includes('T')) {
		// The end date is exclusive
		const last = new Date(end.getFullYear(), end.getMonth(), end.getDate() - 1);
		return [date.formatRange(start, last), 'All day'];
	}
	const time = new Intl.DateTimeFormat(locale, { hour: '2-digit', minute: '2-digit' });
	const sameDay =
		o.start.slice(0, 10) === o.end.slice(0, 10) ||
		(o.end.slice(11, 16) === '00:00' && end.getTime() - start.getTime() < 86_400_000);
	if (sameDay) return [date.format(start), time.formatRange(start, end)];
	const dateTime = new Intl.DateTimeFormat(locale, {
		weekday: 'long',
		day: 'numeric',
		month: 'long',
		hour: '2-digit',
		minute: '2-digit'
	});
	return [dateTime.format(start) + ' –', dateTime.format(end)];
}

const every = { daily: 'day', weekly: 'week', monthly: 'month', yearly: 'year' };

/** The popover's line for a series: `Every week until 31 December 2026` */
export function describeRepeat(repeat: Repeat, locale = displayLocale): string {
	if (repeat.frequency === 'custom') return 'Repeats on a custom schedule';
	const often = `Every ${every[repeat.frequency]}`;
	if (repeat.until === null) return often;
	const date = new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'long', year: 'numeric' });
	return `${often} until ${date.format(wallClock(repeat.until))}`;
}
