// Calendars, occurrences and events through the Tauri commands of docs/architecture.md §10. The
// shapes mirror `simplecalendar_core::service::{CalendarInfo, CalendarUpdate, EventDetail,
// EventDraft, Repeat}` and `simplecalendar_core::model::Occurrence`.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SourceKind } from './sources.js';

export interface CalendarInfo {
	id: number;
	source_id: string;
	source_kind: SourceKind;
	/** A Google account's address, a feed's name */
	source_name: string;
	name: string;
	/** The user's colour, else the provider's: a `#rrggbb` value */
	color: string | null;
	editable: boolean;
	visible: boolean;
}

export interface Occurrence {
	/** For an occurrence of a series, the series' id */
	event_id: number;
	/** With `event_id`, identifies the occurrence */
	original_start: number;
	calendar_id: number;
	title: string;
	location: string | null;
	color: string | null;
	/** `2026-10-20T09:00:00+02:00`, a wall-clock time in the zone passed to `listOccurrences`, or
	 * `2026-10-20` for an all-day occurrence */
	start: string;
	/** Exclusive. For an all-day occurrence the day after its last day. */
	end: string;
	editable: boolean;
}

/** Fields left out stay as they are */
export interface CalendarUpdate {
	/** Only for a local calendar; trimmed, must not be empty */
	name?: string;
	visible?: boolean;
	/** A `#rrggbb` value, or `null` for the provider's colour */
	color_override?: string | null;
}

export interface EventReminder {
	/** 0 to 40320 (four weeks) */
	minutes_before: number;
	/** Only `popup` reminders notify; `email` ones are Google's and kept as they are */
	method: 'popup' | 'email';
}

/** How often an event repeats; `custom` is a rule the editor cannot show, kept as it is */
export type Frequency = 'daily' | 'weekly' | 'monthly' | 'yearly' | 'custom';

export interface Repeat {
	frequency: Frequency;
	/** The last day an occurrence may start on, `YYYY-MM-DD`; `null` repeats forever. Always
	 * `null` for `custom`. */
	until: string | null;
}

/** One event in full, or one occurrence of a series, from `getEvent` */
export interface EventDetail {
	/** For an occurrence of a series, the series' id */
	id: number;
	calendar_id: number;
	title: string;
	/** Plain text, never HTML */
	description: string | null;
	location: string | null;
	all_day: boolean;
	/** The wall clock in `time_zone`, `2026-10-20T09:00`, or `2026-10-20` all-day */
	start: string;
	/** Exclusive: an all-day end is the day after the last day */
	end: string;
	/** IANA name, or an offset such as `+05:30`; `null` for all-day and floating times */
	time_zone: string | null;
	/** The calendar's defaults apply; `reminders` lists them */
	use_default_reminders: boolean;
	/** Earliest first */
	reminders: EventReminder[];
	editable: boolean;
	/** The series' rule, also for one of its occurrences; `null` for a single event */
	repeat: Repeat | null;
}

/** A whole event, as `createEvent` and `updateEvent` take it */
export interface EventDraft {
	calendar_id: number;
	/** A blank title becomes "New event" */
	title: string;
	description: string | null;
	location: string | null;
	all_day: boolean;
	/** `YYYY-MM-DDTHH:MM` in `time_zone`, or `YYYY-MM-DD` all-day */
	start: string;
	/** Like `start`; exclusive for all-day */
	end: string;
	/** IANA name; needed unless all-day */
	time_zone: string | null;
	use_default_reminders: boolean;
	reminders: EventReminder[];
	/** `null` for an event that does not repeat */
	repeat: Repeat | null;
}

/** Names an occurrence: the `event_id` and `original_start` of an `Occurrence` */
export type OccurrenceRef = Pick<Occurrence, 'event_id' | 'original_start'>;

/** Which part of a series a change applies to; the same for a single event */
export type Scope = 'this' | 'all';

/** Payload of `calendar:changed` */
export interface CalendarChanged {
	calendar_ids: number[];
}

/** The zone of this computer; Rust returns every time in it. */
export const displayZone = () => Intl.DateTimeFormat().resolvedOptions().timeZone;

/** Occurrences from date `from` up to, not including, date `to` (both `YYYY-MM-DD`) */
export const listOccurrences = (from: string, to: string) =>
	invoke<Occurrence[]>('list_occurrences', { from, to, tz: displayZone() });
export const listCalendars = () => invoke<CalendarInfo[]>('list_calendars');
/** Returns the calendar as it is now; `calendar:changed` follows */
export const updateCalendar = (calendar: number, update: CalendarUpdate) =>
	invoke<CalendarInfo>('update_calendar', { calendar, update });

/** A new calendar on this computer; `calendar:changed` follows */
export const createLocalCalendar = (name: string) =>
	invoke<CalendarInfo>('create_local_calendar', { name });
/** Deletes a local calendar with its events; `calendar:changed` follows */
export const removeCalendar = (calendar: number) => invoke<void>('remove_calendar', { calendar });

/** An event, or the occurrence of a series that `target` names, as that occurrence shows */
export const getEvent = (target: OccurrenceRef) =>
	invoke<EventDetail>('get_event', {
		target: { event_id: target.event_id, original_start: target.original_start }
	});
/** Returns the new occurrence; `calendar:changed` follows */
export const createEvent = (draft: EventDraft) =>
	invoke<Occurrence>('create_event', { draft, tz: displayZone() });
/**
 * Replaces the event, or one or all occurrences of a series, with `draft`. Returns the changed
 * occurrence, or `null` when a change to all events leaves none at that time; `calendar:changed`
 * follows.
 */
export const updateEvent = (target: OccurrenceRef, draft: EventDraft, scope: Scope = 'all') =>
	invoke<Occurrence | null>('update_event', {
		target: { event_id: target.event_id, original_start: target.original_start },
		scope,
		draft,
		tz: displayZone()
	});
/** `calendar:changed` follows */
export const deleteEvent = (target: OccurrenceRef, scope: Scope = 'all') =>
	invoke<void>('delete_event', {
		target: { event_id: target.event_id, original_start: target.original_start },
		scope
	});

export const onCalendarChanged = (
	handler: (changed: CalendarChanged) => void
): Promise<UnlistenFn> =>
	listen<CalendarChanged>('calendar:changed', (event) => handler(event.payload));

/** Identifies an occurrence, also when it is drawn in pieces on several days */
export const occurrenceKey = (o: Occurrence) => `${o.event_id}:${o.original_start}`;
