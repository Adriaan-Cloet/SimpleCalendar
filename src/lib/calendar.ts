// Calendars and occurrences through the Tauri commands of docs/architecture.md §10. The shapes
// mirror `simplecalendar_core::service::{CalendarInfo, CalendarUpdate}` and
// `simplecalendar_core::model::Occurrence`.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SourceKind } from './sources.js';

export interface CalendarInfo {
	id: number;
	source_id: string;
	source_kind: SourceKind;
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
	visible?: boolean;
	/** A `#rrggbb` value, or `null` for the provider's colour */
	color_override?: string | null;
}

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

export const onCalendarChanged = (
	handler: (changed: CalendarChanged) => void
): Promise<UnlistenFn> =>
	listen<CalendarChanged>('calendar:changed', (event) => handler(event.payload));

/** Identifies an occurrence, also when it is drawn in pieces on several days */
export const occurrenceKey = (o: Occurrence) => `${o.event_id}:${o.original_start}`;
