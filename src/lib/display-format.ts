// How the app writes dates and times (design.md §4 "Dates and times"): the day first and a
// 24-hour clock, in English, whatever locale the system and so the webview report. Every
// `Intl` formatter in the views takes `displayLocale`; the editor's fields use the functions
// below instead of the native date and time inputs, which always follow the system locale.
// Nothing here converts between zones: values are wall-clock dates and times as written.

/** English with day-first dates and a 24-hour clock: "Tuesday 20 October", "09:00" */
export const displayLocale = 'en-GB';

/** `YYYY-MM-DD` as the date field shows it: `20/10/2026` */
export function formatDateField(date: string): string {
	const [y, m, d] = date.split('-');
	return `${d}/${m}/${y}`;
}

/**
 * A typed date as `YYYY-MM-DD`, or `null`. Day first, separated by `/`, `-`, `.` or a space;
 * a two-digit year is this century, a missing one is `year`. `YYYY-MM-DD` is accepted as well.
 */
export function parseDateField(text: string, year: number): string | null {
	const value = text.trim();
	const iso = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(value);
	const dayFirst = /^(\d{1,2})[/.\- ](\d{1,2})(?:[/.\- ](\d{2}|\d{4}))?$/.exec(value);
	let parts: [number, number, number];
	if (iso) parts = [Number(iso[1]), Number(iso[2]), Number(iso[3])];
	else if (dayFirst) {
		const y = dayFirst[3] === undefined ? year : Number(dayFirst[3]);
		parts = [y < 100 ? 2000 + y : y, Number(dayFirst[2]), Number(dayFirst[1])];
	} else return null;
	const [y, m, d] = parts;
	const date = new Date(Date.UTC(y, m - 1, d));
	// Date.UTC rolls 31 February over into March: such a date does not exist
	if (date.getUTCFullYear() !== y || date.getUTCMonth() !== m - 1 || date.getUTCDate() !== d) {
		return null;
	}
	return date.toISOString().slice(0, 10);
}

/** A typed time as `HH:MM`, or `null`: `9`, `9:30`, `09.30`, `21h30`, `0930` */
export function parseTimeField(text: string): string | null {
	const match = /^(\d{1,2})(?:[:.h]?(\d{2}))?$/.exec(text.trim());
	if (!match) return null;
	const hours = Number(match[1]);
	const minutes = Number(match[2] ?? 0);
	if (hours > 23 || minutes > 59) return null;
	return `${pad(hours)}:${pad(minutes)}`;
}

/**
 * What the time menu offers: every quarter of an hour, or from `from` on in quarters, keeping
 * its minutes, until the end of that day. The end time's menu starts at the start time.
 */
export function timeOptions(from: string | null): string[] {
	const [h, m] = (from ?? '00:00').split(':').map(Number);
	const options: string[] = [];
	for (let minutes = h * 60 + m; minutes < 1440; minutes += 15) {
		options.push(`${pad(Math.floor(minutes / 60))}:${pad(minutes % 60)}`);
	}
	return options;
}

/** A length of time as the end time's menu shows it: `30 min`, `1 h`, `1 h 30 min` */
export function durationLabel(minutes: number): string {
	const hours = Math.floor(minutes / 60);
	const rest = minutes % 60;
	if (hours === 0) return `${rest} min`;
	return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}

function pad(n: number): string {
	return String(n).padStart(2, '0');
}
