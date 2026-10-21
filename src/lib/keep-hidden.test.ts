import { describe, expect, it } from 'vitest';
import type { Occurrence } from './calendar.js';
import { keepHidden } from './keep-hidden.js';

function occurrence(event_id: number, calendar_id: number, start: string): Occurrence {
	return {
		event_id,
		original_start: 0,
		calendar_id,
		title: String(event_id),
		location: null,
		color: null,
		start,
		end: start,
		editable: false
	};
}

const ids = (list: Occurrence[]) => list.map((o) => o.event_id);

describe('keepHidden', () => {
	const work = occurrence(1, 1, '2026-10-20T09:00:00+02:00');
	const gym = occurrence(2, 2, '2026-10-20T08:00:00+02:00');
	const holiday = occurrence(3, 2, '2026-10-20');

	it('keeps what was on screen of a calendar that is hidden now', () => {
		// Calendar 2 was just hidden: Rust no longer returns its occurrences
		expect(ids(keepHidden([gym, work, holiday], [work], new Set([2])))).toEqual([3, 2, 1]);
	});

	it('sorts the merged list by start', () => {
		const later = occurrence(4, 1, '2026-10-21T07:00:00+02:00');
		expect(ids(keepHidden([gym], [work, later], new Set([2])))).toEqual([2, 1, 4]);
	});

	it('drops what Rust returns again once the calendar is shown', () => {
		expect(ids(keepHidden([gym, work], [gym, work], new Set()))).toEqual([2, 1]);
	});

	it('keeps nothing of a visible calendar that Rust no longer returns', () => {
		// The event was deleted in a sync
		expect(ids(keepHidden([gym, work], [work], new Set([3])))).toEqual([1]);
	});
});
