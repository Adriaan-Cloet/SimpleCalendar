// Written before `placeMonth` and `fitDay`: which events each day of the month view lists, and
// how many of them fit before "+N more".
import { describe, expect, it } from 'vitest';
import type { Occurrence } from './calendar.js';
import { fitDay, placeMonth } from './month-layout.js';

// ---- helpers -------------------------------------------------------------------------------

/** Monday 19 to Sunday 25 October 2026: enough days to show what a month does */
const DAYS = [
	'2026-10-19',
	'2026-10-20',
	'2026-10-21',
	'2026-10-22',
	'2026-10-23',
	'2026-10-24',
	'2026-10-25'
];

let next = 1;

function occ(start: string, end: string, title = start): Occurrence {
	return {
		event_id: next++,
		original_start: 0,
		calendar_id: 1,
		title,
		location: null,
		color: null,
		start,
		end,
		editable: false
	};
}

/** A timed value on `date` at `time`, with the offset Rust would add */
const at = (date: string, time: string) => `${date}T${time}:00+02:00`;

/** Per day, `title` for a timed item and `[title]` for an all-day one */
function lists(occurrences: Occurrence[]) {
	return placeMonth(occurrences, DAYS).map((items) =>
		items.map((i) => (i.allDay ? [i.occurrence.title] : i.occurrence.title))
	);
}

// ---- placeMonth ----------------------------------------------------------------------------

describe('placeMonth', () => {
	it('lists nothing without events', () => {
		expect(lists([])).toEqual([[], [], [], [], [], [], []]);
	});

	it('lists a timed event on its day, in the order Rust sorted them', () => {
		const found = lists([
			occ(at('2026-10-20', '09:00'), at('2026-10-20', '09:30'), 'Standup'),
			occ(at('2026-10-20', '11:00'), at('2026-10-20', '12:00'), 'Review'),
			occ(at('2026-10-23', '14:00'), at('2026-10-23', '15:00'), 'Call')
		]);
		expect(found[1]).toEqual(['Standup', 'Review']);
		expect(found[4]).toEqual(['Call']);
	});

	it('lists a timed event that runs past midnight only on the day it starts', () => {
		const found = lists([occ(at('2026-10-23', '22:00'), at('2026-10-24', '02:00'), 'Party')]);
		expect(found[4]).toEqual(['Party']);
		expect(found[5]).toEqual([]);
	});

	it('lists an all-day event on every one of its days', () => {
		// The end date is exclusive: 21 to 23 October
		const found = lists([occ('2026-10-21', '2026-10-24', 'Conference')]);
		expect(found.slice(2, 5)).toEqual([[['Conference']], [['Conference']], [['Conference']]]);
		expect(found[1]).toEqual([]);
		expect(found[5]).toEqual([]);
	});

	it('treats a timed event of 24 hours or more as all-day', () => {
		const found = lists([occ(at('2026-10-20', '12:00'), at('2026-10-22', '12:00'), 'Trip')]);
		expect(found.slice(1, 4)).toEqual([[['Trip']], [['Trip']], [['Trip']]]);
	});

	it('puts all-day events before timed ones', () => {
		const found = lists([
			occ(at('2026-10-20', '08:00'), at('2026-10-20', '09:00'), 'Early'),
			occ('2026-10-20', '2026-10-21', 'Birthday')
		]);
		expect(found[1]).toEqual([['Birthday'], 'Early']);
	});

	it('keeps all-day events in one order on every day they share', () => {
		const found = lists([
			occ('2026-10-21', '2026-10-22', 'Short'),
			occ('2026-10-20', '2026-10-23', 'Long')
		]);
		expect(found[1]).toEqual([['Long']]);
		expect(found[2]).toEqual([['Long'], ['Short']]);
		expect(found[3]).toEqual([['Long']]);
	});

	it('lists events that started before the first day shown', () => {
		const found = lists([occ('2026-10-15', '2026-10-21', 'Holiday')]);
		expect(found[0]).toEqual([['Holiday']]);
		expect(found[1]).toEqual([['Holiday']]);
		expect(found[2]).toEqual([]);
	});
});

// ---- fitDay --------------------------------------------------------------------------------

describe('fitDay', () => {
	it('shows everything that fits', () => {
		expect(fitDay(3, 4)).toEqual({ shown: 3, more: 0 });
		expect(fitDay(4, 4)).toEqual({ shown: 4, more: 0 });
	});

	it('gives the last line to "+N more" when not everything fits', () => {
		expect(fitDay(5, 4)).toEqual({ shown: 3, more: 2 });
		expect(fitDay(9, 2)).toEqual({ shown: 1, more: 8 });
	});

	it('shows only "+N more" when one line fits', () => {
		expect(fitDay(3, 1)).toEqual({ shown: 0, more: 3 });
	});

	it('shows nothing when no line fits, before the cell is measured', () => {
		expect(fitDay(3, 0)).toEqual({ shown: 0, more: 0 });
		expect(fitDay(0, 0)).toEqual({ shown: 0, more: 0 });
	});
});
