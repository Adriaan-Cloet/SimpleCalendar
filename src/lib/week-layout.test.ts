// Written before `placeWeek`: how occurrences from Rust land on the days of the week view.
import { describe, expect, it } from 'vitest';
import type { Occurrence } from './calendar.js';
import { placeWeek } from './week-layout.js';

// ---- helpers -------------------------------------------------------------------------------

const WEEK = [
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

/** `[day, start, end]` per title, for the timed pieces */
function timed(occurrences: Occurrence[]) {
	return placeWeek(occurrences, WEEK).timed.flatMap((pieces, day) =>
		pieces.map((p) => [p.occurrence.title, day, p.start, p.end])
	);
}

/** `[first, last, row]` per title, for the all-day bars */
function bars(occurrences: Occurrence[]) {
	return placeWeek(occurrences, WEEK).allDay.map((b) => [
		b.occurrence.title,
		b.first,
		b.last,
		b.row
	]);
}

// ---- timed ---------------------------------------------------------------------------------

describe('placeWeek, timed', () => {
	it('puts a timed occurrence on its day in minutes since midnight', () => {
		expect(timed([occ(at('2026-10-20', '09:00'), at('2026-10-20', '10:30'), 'a')])).toEqual([
			['a', 1, 540, 630]
		]);
	});

	it('reads the wall-clock time as written, whatever the offset', () => {
		const o = occ('2026-10-25T02:30:00+01:00', '2026-10-25T03:00:00+01:00', 'a');
		expect(timed([o])).toEqual([['a', 6, 150, 180]]);
	});

	it('draws an occurrence past midnight on both days, relative to each day', () => {
		expect(timed([occ(at('2026-10-20', '22:00'), at('2026-10-21', '02:00'), 'a')])).toEqual([
			['a', 1, 1320, 1560],
			['a', 2, -120, 120]
		]);
	});

	it('does not draw an occurrence that ends at midnight on the next day', () => {
		expect(timed([occ(at('2026-10-20', '23:00'), at('2026-10-21', '00:00'), 'a')])).toEqual([
			['a', 1, 1380, 1440]
		]);
	});

	it('keeps an occurrence without length', () => {
		expect(timed([occ(at('2026-10-20', '00:00'), at('2026-10-20', '00:00'), 'a')])).toEqual([
			['a', 1, 0, 0]
		]);
	});

	it('draws only the days inside the week', () => {
		expect(timed([occ(at('2026-10-18', '23:00'), at('2026-10-19', '01:00'), 'a')])).toEqual([
			['a', 0, -60, 60]
		]);
		expect(timed([occ(at('2026-10-25', '23:00'), at('2026-10-26', '01:00'), 'b')])).toEqual([
			['b', 6, 1380, 1500]
		]);
	});

	it('moves an occurrence of 24 hours or more to the all-day row', () => {
		const long = [
			occ(at('2026-10-20', '09:00'), at('2026-10-21', '09:00'), 'day'),
			occ(at('2026-10-20', '09:00'), at('2026-10-22', '17:00'), 'trip'),
			occ(at('2026-10-20', '00:00'), at('2026-10-22', '00:00'), 'midnight')
		];
		expect(timed(long)).toEqual([]);
		expect(bars(long)).toEqual([
			['trip', 1, 3, 0],
			['midnight', 1, 2, 1],
			['day', 1, 2, 2]
		]);
	});
});

// ---- all-day -------------------------------------------------------------------------------

describe('placeWeek, all-day', () => {
	it('spans an all-day occurrence over its days; the end date is exclusive', () => {
		expect(
			bars([occ('2026-10-20', '2026-10-21', 'one'), occ('2026-10-21', '2026-10-24', 'three')])
		).toEqual([
			['one', 1, 1, 0],
			['three', 2, 4, 0]
		]);
	});

	it('clips to the week', () => {
		expect(bars([occ('2026-10-01', '2026-11-01', 'month')])).toEqual([['month', 0, 6, 0]]);
		expect(bars([occ('2026-10-25', '2026-10-28', 'end')])).toEqual([['end', 6, 6, 0]]);
	});

	it('leaves out what falls outside the week', () => {
		expect(bars([occ('2026-10-26', '2026-10-27'), occ('2026-10-18', '2026-10-19')])).toEqual([]);
	});

	it('stacks overlapping bars in rows: earliest, longest, then earliest start first; free rows are reused', () => {
		expect(
			bars([
				occ('2026-10-21', '2026-10-22', 'short'),
				occ('2026-10-19', '2026-10-23', 'long'),
				occ('2026-10-21', '2026-10-23', 'mid'),
				occ('2026-10-24', '2026-10-25', 'weekend')
			])
		).toEqual([
			['long', 0, 3, 0],
			['mid', 2, 3, 1],
			['short', 2, 2, 2],
			['weekend', 5, 5, 0]
		]);
	});

	it('counts the rows', () => {
		expect(placeWeek([], WEEK).rows).toBe(0);
		expect(
			placeWeek([occ('2026-10-20', '2026-10-21'), occ('2026-10-20', '2026-10-22')], WEEK).rows
		).toBe(2);
	});
});

describe('a single day', () => {
	const DAY = ['2026-10-21'];

	it('keeps only that day’s piece of an event across midnight', () => {
		const late = occ(at('2026-10-20', '22:00'), at('2026-10-21', '02:00'), 'late');
		const { timed } = placeWeek([late], DAY);
		expect(timed).toEqual([[{ occurrence: late, start: -120, end: 120 }]]);
	});

	it('shows an all-day event that spans it in the one column', () => {
		const trip = occ('2026-10-19', '2026-10-24', 'trip');
		const layout = placeWeek([trip], DAY);
		expect(layout.allDay).toEqual([{ occurrence: trip, first: 0, last: 0, row: 0 }]);
		expect(layout.rows).toBe(1);
	});

	it('leaves out what falls on other days', () => {
		const layout = placeWeek(
			[occ(at('2026-10-22', '09:00'), at('2026-10-22', '10:00')), occ('2026-10-20', '2026-10-21')],
			DAY
		);
		expect(layout.timed).toEqual([[]]);
		expect(layout.allDay).toEqual([]);
	});
});
