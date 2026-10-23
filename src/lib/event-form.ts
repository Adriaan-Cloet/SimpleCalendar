// The event editor's form (issue #16): what it shows, and how that becomes an `EventDraft`.
// Everything here works on wall-clock dates and times as written. Turning them into instants,
// with the event's zone and its DST rules, is Rust's job (architecture §10).
import type {
	CalendarInfo,
	EventDetail,
	EventDraft,
	EventReminder,
	Frequency,
	Scope
} from './calendar.js';
import { inRange, type ViewRange } from './view-range.js';
import { isoDate } from './week-dates.js';

export interface EventForm {
	calendarId: number;
	title: string;
	location: string;
	notes: string;
	allDay: boolean;
	/** `YYYY-MM-DD` */
	startDate: string;
	/** `HH:MM`; kept while the event is all-day, for when it stops being so */
	startTime: string;
	/** For an all-day event the last day, inclusive, as the editor shows it */
	endDate: string;
	endTime: string;
	/** IANA name the times are in */
	timeZone: string;
	useDefaultReminders: boolean;
	reminders: EventReminder[];
	repeat: 'never' | Frequency;
	/** The last day it repeats on, inclusive; `null` repeats forever. Kept while `repeat` is
	 * `never` or `custom`, but not sent. */
	repeatUntil: string | null;
}

/** The times an all-day event offers when it is switched to timed */
const DEFAULT_TIMES = { startTime: '09:00', endTime: '10:00' };

export function newEventForm(options: {
	calendarId: number;
	/** The local wall-clock start */
	start: Date;
	allDay: boolean;
	timeZone: string;
}): EventForm {
	const { calendarId, start, allDay, timeZone } = options;
	const end = new Date(start.getTime());
	end.setHours(end.getHours() + 1);
	const times = allDay
		? { endDate: isoDate(start), ...DEFAULT_TIMES }
		: { endDate: isoDate(end), startTime: clock(start), endTime: clock(end) };
	return {
		calendarId,
		title: '',
		location: '',
		notes: '',
		allDay,
		startDate: isoDate(start),
		timeZone,
		useDefaultReminders: false,
		reminders: [],
		repeat: 'never',
		repeatUntil: null,
		...times
	};
}

/** `displayZone` stands in for an event without a zone of its own: all-day or floating */
export function formFromEvent(event: EventDetail, displayZone: string): EventForm {
	const timed = event.start.includes('T');
	return {
		calendarId: event.calendar_id,
		title: event.title,
		location: event.location ?? '',
		notes: event.description ?? '',
		allDay: event.all_day,
		startDate: event.start.slice(0, 10),
		endDate: timed ? event.end.slice(0, 10) : shiftDate(event.end, -1),
		...(timed
			? { startTime: event.start.slice(11, 16), endTime: event.end.slice(11, 16) }
			: DEFAULT_TIMES),
		timeZone: event.time_zone ?? displayZone,
		useDefaultReminders: event.use_default_reminders,
		reminders: event.reminders.map((r) => ({ ...r })),
		repeat: event.repeat?.frequency ?? 'never',
		repeatUntil: event.repeat?.until ?? null
	};
}

export function draftFromForm(form: EventForm): EventDraft {
	const text = (value: string) => (value.trim() === '' ? null : value);
	return {
		calendar_id: form.calendarId,
		title: form.title,
		description: text(form.notes),
		location: text(form.location),
		all_day: form.allDay,
		start: form.allDay ? form.startDate : `${form.startDate}T${form.startTime}`,
		end: form.allDay ? shiftDate(form.endDate, 1) : `${form.endDate}T${form.endTime}`,
		time_zone: form.allDay ? null : form.timeZone,
		use_default_reminders: form.useDefaultReminders,
		reminders: form.reminders,
		repeat:
			form.repeat === 'never'
				? null
				: { frequency: form.repeat, until: hasRepeatEnd(form) ? form.repeatUntil : null }
	};
}

/** Whether the form's repeat has a last day that counts */
function hasRepeatEnd(form: EventForm): boolean {
	return form.repeat !== 'never' && form.repeat !== 'custom' && form.repeatUntil !== null;
}

/** Repeating until a last day, a month after the start unless one was chosen, or forever */
export function withRepeatEnd(form: EventForm, ends: boolean): EventForm {
	if (!ends) return { ...form, repeatUntil: null };
	if (form.repeatUntil !== null) return form;
	const [y, m, d] = form.startDate.split('-').map(Number);
	// Day 0 of the month after next is the last day of next month: a shorter month clamps there
	const last = new Date(Date.UTC(y, m + 1, 0)).getUTCDate();
	const until = new Date(Date.UTC(y, m, Math.min(d, last)));
	return { ...form, repeatUntil: until.toISOString().slice(0, 10) };
}

/**
 * What saving a changed occurrence of a series may apply to. The rule and the calendar belong
 * to the whole series, so a change to either applies to all events (architecture §10).
 */
export function saveScopes(initial: EventForm, form: EventForm): Scope[] {
	const same =
		form.repeat === initial.repeat &&
		form.repeatUntil === initial.repeatUntil &&
		form.calendarId === initial.calendarId;
	return same ? ['this', 'all'] : ['all'];
}

/** A new start that keeps the event's length on the wall clock, as most calendars do */
export function withStart(form: EventForm, date: string, time: string): EventForm {
	const next = { ...form, startDate: date, startTime: time };
	const before = minutes(form.startDate, form.allDay ? '00:00' : form.startTime);
	const after = minutes(date, form.allDay ? '00:00' : time);
	const end = minutes(form.endDate, form.allDay ? '00:00' : form.endTime);
	if (before === null || after === null || end === null) return next;
	const moved = new Date((end + after - before) * 60_000);
	return {
		...next,
		endDate: moved.toISOString().slice(0, 10),
		endTime: form.allDay ? form.endTime : moved.toISOString().slice(11, 16)
	};
}

/** Times are kept while all-day, so switching back restores them */
export function withAllDay(form: EventForm, allDay: boolean): EventForm {
	return { ...form, allDay };
}

/** Why the form cannot be saved, or `null`. Rust checks again. */
export function formProblem(form: EventForm): string | null {
	const start = minutes(form.startDate, form.allDay ? '00:00' : form.startTime);
	const end = minutes(form.endDate, form.allDay ? '00:00' : form.endTime);
	if (!validDate(form.startDate) || !validDate(form.endDate)) {
		return 'Enter dates as day/month/year, such as 20/10/2026.';
	}
	if (start === null || end === null)
		return 'Enter times on a 24-hour clock, such as 09:30 or 21:30.';
	if (end < start) return 'The event ends before it starts.';
	if (hasRepeatEnd(form)) {
		if (!validDate(form.repeatUntil!)) return 'Enter dates as day/month/year, such as 20/10/2026.';
		if (form.repeatUntil! < form.startDate) {
			return 'The event must stop repeating on or after the day it starts.';
		}
	}
	return null;
}

/** What the reminder menu offers, in minutes before the start */
export const reminderChoices = [0, 5, 10, 15, 30, 60, 120, 1440, 2880, 10080];

export function reminderLabel(minutesBefore: number): string {
	if (minutesBefore === 0) return 'At time of event';
	const units: [number, string][] = [
		[10080, 'week'],
		[1440, 'day'],
		[60, 'hour'],
		[1, 'minute']
	];
	const [size, unit] = units.find(([size]) => minutesBefore % size === 0)!;
	const count = minutesBefore / size;
	return `${count} ${unit}${count === 1 ? '' : 's'} before`;
}

/** A `YYYY-MM-DD` date `days` later, on the calendar: no zones involved */
export function shiftDate(date: string, days: number): string {
	const [y, m, d] = date.split('-').map(Number);
	return new Date(Date.UTC(y, m - 1, d + days)).toISOString().slice(0, 10);
}

/** Where "New event" starts: the next whole hour if today is on screen, else 09:00 on the first
 * day shown */
export function defaultStart(range: ViewRange, now: Date): Date {
	if (inRange(range, now)) {
		return new Date(now.getFullYear(), now.getMonth(), now.getDate(), now.getHours() + 1);
	}
	const { start } = range;
	return new Date(start.getFullYear(), start.getMonth(), start.getDate(), 9);
}

/**
 * The calendars the editor offers: every writable one for a new event (`current` is `null`).
 * An existing event moves only between calendars on this computer; one of an account stays in
 * its calendar, as Rust requires (architecture §10).
 */
export function calendarChoices(calendars: CalendarInfo[], current: number | null): CalendarInfo[] {
	const writable = calendars.filter((c) => c.editable);
	if (current === null) return writable;
	const own = calendars.find((c) => c.id === current);
	if (own?.source_kind !== 'local') return writable.filter((c) => c.id === current);
	return writable.filter((c) => c.source_kind === 'local');
}

/** The zones the editor offers: what the webview knows, plus the event's and the viewer's */
export function zoneOptions(supported: string[], current: string, display: string): string[] {
	return [...new Set([...supported, current, display, 'UTC'])].sort();
}

/** `HH:MM` of a local `Date` */
function clock(date: Date): string {
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function validDate(date: string): boolean {
	if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return false;
	return shiftDate(date, 0) === date;
}

/** Minutes since 1970 of a wall-clock date and time, as if in UTC; `null` if unreadable */
function minutes(date: string, time: string): number | null {
	if (!validDate(date) || !/^([01]\d|2[0-3]):[0-5]\d$/.test(time)) return null;
	const [y, m, d] = date.split('-').map(Number);
	const [h, min] = time.split(':').map(Number);
	return Date.UTC(y, m - 1, d, h, min) / 60_000;
}
