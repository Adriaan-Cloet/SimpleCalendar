// The sidebar's calendar groups: the local calendars, then one group per Google account named
// after it, then CalDAV, then all subscriptions together. Calendars keep the order
// `list_calendars` gave them (primary first, then by name).
import type { CalendarInfo } from './calendar.js';
import type { SourceKind } from './sources.js';

export interface CalendarGroup {
	/** Stable key for `#each` */
	key: string;
	label: string;
	calendars: CalendarInfo[];
}

// In this order. `null`: one group per source, named after it.
const kinds: [SourceKind, string | null][] = [
	['local', 'On this computer'],
	['google', null],
	['caldav', null],
	['ics', 'Subscriptions']
];

export function groupCalendars(calendars: CalendarInfo[]): CalendarGroup[] {
	return kinds.flatMap(([kind, label]) => {
		const ofKind = calendars.filter((c) => c.source_kind === kind);
		if (ofKind.length === 0) return [];
		if (label !== null) return [{ key: kind, label, calendars: ofKind }];
		const sources = [...new Set(ofKind.map((c) => c.source_id))];
		return sources.map((source) => {
			const own = ofKind.filter((c) => c.source_id === source);
			return { key: source, label: own[0].source_name, calendars: own };
		});
	});
}
