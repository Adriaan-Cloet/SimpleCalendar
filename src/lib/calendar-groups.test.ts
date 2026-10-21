import { describe, expect, it } from 'vitest';
import type { CalendarInfo } from './calendar.js';
import { groupCalendars } from './calendar-groups.js';
import type { SourceKind } from './sources.js';

function calendar(id: number, kind: SourceKind, source: string, sourceName: string): CalendarInfo {
	return {
		id,
		source_id: source,
		source_kind: kind,
		source_name: sourceName,
		name: `Calendar ${id}`,
		color: null,
		editable: false,
		visible: true
	};
}

const summary = (calendars: CalendarInfo[]) =>
	groupCalendars(calendars).map((g) => [g.label, g.calendars.map((c) => c.id)]);

describe('groupCalendars', () => {
	it('gives every Google account its own group, named after it', () => {
		expect(
			summary([
				calendar(1, 'google', 'a', 'me@example.com'),
				calendar(2, 'google', 'a', 'me@example.com'),
				calendar(3, 'google', 'b', 'work@example.com')
			])
		).toEqual([
			['me@example.com', [1, 2]],
			['work@example.com', [3]]
		]);
	});

	it('puts all subscriptions in one group, after the accounts', () => {
		expect(
			summary([
				calendar(1, 'ics', 'f1', 'Holidays'),
				calendar(2, 'google', 'a', 'me@example.com'),
				calendar(3, 'ics', 'f2', 'Sports')
			])
		).toEqual([
			['me@example.com', [2]],
			['Subscriptions', [1, 3]]
		]);
	});

	it('leaves out kinds without calendars and keeps the order within a group', () => {
		expect(
			summary([calendar(5, 'local', 'l', 'Local'), calendar(4, 'local', 'l', 'Local')])
		).toEqual([['On this computer', [5, 4]]]);
		expect(summary([])).toEqual([]);
	});
});
