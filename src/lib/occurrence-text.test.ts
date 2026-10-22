import { describe, expect, it } from 'vitest';
import type { Occurrence } from './calendar.js';
import { describeRepeat, describeWhen, wallClock } from './occurrence-text.js';

function occ(start: string, end: string): Occurrence {
	return {
		event_id: 1,
		original_start: 0,
		calendar_id: 1,
		title: 'Event',
		location: null,
		color: null,
		start,
		end,
		editable: false
	};
}

// The exact punctuation and spacing come from ICU and differ between versions, so the cases
// check which parts each line holds rather than whole strings.
const when = (o: Occurrence) => describeWhen(o, 'en-GB');

describe('wallClock', () => {
	it('keeps the written date and time, whatever the offset', () => {
		const d = wallClock('2026-10-25T02:30:00-05:00');
		expect([d.getFullYear(), d.getMonth(), d.getDate(), d.getHours(), d.getMinutes()]).toEqual([
			2026, 9, 25, 2, 30
		]);
	});

	it('reads a date as its midnight', () => {
		const d = wallClock('2026-10-20');
		expect([d.getDate(), d.getHours()]).toEqual([20, 0]);
	});
});

describe('describeWhen', () => {
	it('gives the date and the time range of a timed occurrence', () => {
		const [date, time] = when(occ('2026-10-20T09:00:00+02:00', '2026-10-20T10:30:00+02:00'));
		expect(date).toBe('Tuesday 20 October');
		expect(time).toMatch(/^09:00\s*–\s*10:30$/);
	});

	it('treats an end at midnight as the same day', () => {
		expect(when(occ('2026-10-20T23:00:00+02:00', '2026-10-21T00:00:00+02:00'))[0]).toBe(
			'Tuesday 20 October'
		);
	});

	it('gives both dates of a timed occurrence over midnight', () => {
		const [from, to] = when(occ('2026-10-20T22:00:00+02:00', '2026-10-21T02:00:00+02:00'));
		expect(from).toMatch(/^Tuesday 20 October.*22:00 –$/);
		expect(to).toMatch(/^Wednesday 21 October.*2:00$/);
	});

	it('gives the days of an all-day occurrence, the end being exclusive', () => {
		expect(when(occ('2026-10-20', '2026-10-21'))).toEqual(['Tuesday 20 October', 'All day']);
		expect(when(occ('2026-10-20', '2026-10-23'))[0]).toMatch(/^Tuesday 20 .*Thursday 22 October$/);
	});
});

describe('describeRepeat', () => {
	it('names how often, and the last day if there is one', () => {
		expect(describeRepeat({ frequency: 'daily', until: null }, 'en-GB')).toBe('Every day');
		expect(describeRepeat({ frequency: 'yearly', until: null }, 'en-GB')).toBe('Every year');
		expect(describeRepeat({ frequency: 'weekly', until: '2026-12-31' }, 'en-GB')).toBe(
			'Every week until 31 December 2026'
		);
	});

	it('says a rule it cannot show repeats', () => {
		expect(describeRepeat({ frequency: 'custom', until: null }, 'en-GB')).toBe(
			'Repeats on a custom schedule'
		);
	});
});
