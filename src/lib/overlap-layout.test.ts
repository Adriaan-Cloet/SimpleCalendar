// The cases of docs/architecture.md §12 "Frontend: overlap layout", written before the function.
import { describe, expect, it } from 'vitest';
import { DEFAULT_LAYOUT, layoutDay, type Placement, type TimedSpan } from './overlap-layout.js';

// ---- helpers -------------------------------------------------------------------------------

/** Minutes since midnight for "HH:MM"; "-01:00" is an hour before midnight */
function minutes(time: string): number {
	const negative = time.startsWith('-');
	const [h, m] = time.replace('-', '').split(':').map(Number);
	return (negative ? -1 : 1) * (h * 60 + m);
}

function ev(id: string, start: string, end: string): TimedSpan {
	return { id, start: minutes(start), end: minutes(end) };
}

/** `[column, columns, span, indent]` per id, for compact assertions */
function grid(placements: Placement[]): Record<string, [number, number, number, number]> {
	return Object.fromEntries(placements.map((p) => [p.id, [p.column, p.columns, p.span, p.indent]]));
}

const layout = (...events: TimedSpan[]) => layoutDay(events);

// ---- the §12 cases -------------------------------------------------------------------------

describe('layoutDay', () => {
	it('returns nothing for no events', () => {
		expect(layout()).toEqual([]);
	});

	it('gives events that do not overlap the full width', () => {
		expect(grid(layout(ev('a', '09:00', '10:00'), ev('b', '11:00', '12:00')))).toEqual({
			a: [0, 1, 1, 0],
			b: [0, 1, 1, 0]
		});
	});

	it('puts two overlapping events that start together side by side', () => {
		// The longer one comes first
		expect(grid(layout(ev('a', '09:00', '10:00'), ev('b', '09:00', '11:00')))).toEqual({
			b: [0, 2, 1, 0],
			a: [1, 2, 1, 0]
		});
	});

	it('puts two overlapping events side by side when the second would cover the first title', () => {
		expect(grid(layout(ev('a', '09:00', '10:00'), ev('b', '09:10', '10:30')))).toEqual({
			a: [0, 2, 1, 0],
			b: [1, 2, 1, 0]
		});
	});

	it('stacks an event that starts after the earlier title on top of it', () => {
		expect(grid(layout(ev('a', '09:00', '11:00'), ev('b', '09:30', '10:30')))).toEqual({
			a: [0, 1, 1, 0],
			b: [0, 1, 1, 1]
		});
	});

	it('stacks from exactly the title clearance on', () => {
		const clearance = DEFAULT_LAYOUT.titleClearance;
		const result = layoutDay([
			{ id: 'a', start: 540, end: 660 },
			{ id: 'b', start: 540 + clearance, end: 660 },
			{ id: 'c', start: 600, end: 601 + clearance }
		]);
		expect(grid(result).b).toEqual([0, 1, 1, 1]);
		// c starts at least the clearance after both, so it stacks on top of b
		expect(grid(result).c).toEqual([0, 1, 1, 2]);
	});

	it('lets A and C of a chain share a column', () => {
		// A overlaps B, B overlaps C, A does not overlap C
		expect(
			grid(layout(ev('a', '09:00', '10:00'), ev('b', '09:10', '11:00'), ev('c', '10:00', '12:00')))
		).toEqual({
			a: [0, 2, 1, 0],
			b: [1, 2, 1, 0],
			c: [0, 2, 1, 0]
		});
	});

	it('puts three events at the same time in three columns', () => {
		expect(
			grid(layout(ev('c', '09:00', '10:00'), ev('a', '09:00', '10:00'), ev('b', '09:00', '10:00')))
		).toEqual({
			a: [0, 3, 1, 0],
			b: [1, 3, 1, 0],
			c: [2, 3, 1, 0]
		});
	});

	it('does not treat an event that ends when the next starts as an overlap', () => {
		expect(grid(layout(ev('a', '09:00', '10:00'), ev('b', '10:00', '11:00')))).toEqual({
			a: [0, 1, 1, 0],
			b: [0, 1, 1, 0]
		});
	});

	describe('a zero-length event', () => {
		it('gets the minimum height', () => {
			const [p] = layout(ev('a', '10:00', '10:00'));
			expect([p.top, p.bottom]).toEqual([600, 600 + DEFAULT_LAYOUT.minDuration]);
			expect(grid([p]).a).toEqual([0, 1, 1, 0]);
		});

		it('overlaps nothing it merely touches', () => {
			expect(grid(layout(ev('a', '09:00', '10:00'), ev('b', '10:00', '10:00')))).toEqual({
				a: [0, 1, 1, 0],
				b: [0, 1, 1, 0]
			});
			expect(grid(layout(ev('a', '10:00', '10:00'), ev('b', '10:15', '11:00')))).toEqual({
				a: [0, 1, 1, 0],
				b: [0, 1, 1, 0]
			});
		});

		it('overlaps what starts inside its drawn height', () => {
			expect(grid(layout(ev('a', '10:00', '10:00'), ev('b', '10:05', '11:00')))).toEqual({
				b: [1, 2, 1, 0],
				a: [0, 2, 1, 0]
			});
		});
	});

	it('counts columns per overlap group', () => {
		const result = grid(
			layout(
				ev('a', '09:00', '10:00'),
				ev('b', '09:00', '10:00'),
				ev('c', '09:00', '10:00'),
				ev('d', '14:00', '15:00'),
				ev('e', '14:00', '15:00')
			)
		);
		expect([result.a[1], result.b[1], result.c[1]]).toEqual([3, 3, 3]);
		expect([result.d[1], result.e[1]]).toEqual([2, 2]);
	});

	describe('an event that runs past midnight', () => {
		it('is clipped to the day', () => {
			const result = layout(ev('late', '22:00', '26:00'), ev('early', '-01:00', '01:00'));
			expect(result.map((p) => [p.id, p.top, p.bottom])).toEqual([
				['early', 0, 60],
				['late', 1320, 1440]
			]);
		});

		it('is left out on a day it does not reach', () => {
			expect(layout(ev('before', '-02:00', '00:00'), ev('after', '24:00', '25:00'))).toEqual([]);
		});
	});

	it('gives the same result for the same events in another order', () => {
		const events = [
			ev('a', '09:00', '12:00'),
			ev('b', '09:00', '10:00'),
			ev('c', '09:30', '10:30'),
			ev('d', '10:00', '10:00'),
			ev('e', '11:00', '13:00'),
			ev('f', '11:00', '13:00'),
			ev('g', '14:00', '15:00')
		];
		const expected = layoutDay(events);
		for (const order of [
			[...events].reverse(),
			[events[3], events[0], events[6], events[2], events[5], events[1], events[4]],
			[events[6], events[4], events[1], events[5], events[3], events[2], events[0]]
		]) {
			expect(layoutDay(order)).toEqual(expected);
		}
	});

	// ---- beyond the table: the stacking and widening rules ---------------------------------

	it('starts a new column once a stack is as deep as allowed', () => {
		const result = grid(
			layout(
				ev('a', '09:00', '13:00'),
				ev('b', '09:30', '13:00'),
				ev('c', '10:00', '13:00'),
				ev('d', '10:30', '13:00'),
				ev('e', '11:00', '13:00')
			)
		);
		expect(result).toEqual({
			a: [0, 2, 1, 0],
			b: [0, 2, 1, 1],
			c: [0, 2, 1, 2],
			d: [0, 2, 1, 3],
			e: [1, 2, 1, 0]
		});
	});

	it('prefers a free column to stacking', () => {
		expect(
			grid(layout(ev('a', '09:00', '11:00'), ev('b', '09:00', '09:30'), ev('c', '10:00', '10:30')))
		).toEqual({
			a: [0, 2, 1, 0],
			b: [1, 2, 1, 0],
			c: [1, 2, 1, 0]
		});
	});

	it('widens an event into columns that are free for its whole length', () => {
		expect(
			grid(
				layout(
					ev('a', '09:00', '12:00'),
					ev('b', '09:00', '10:00'),
					ev('c', '09:00', '10:00'),
					ev('d', '10:30', '11:30')
				)
			)
		).toEqual({
			a: [0, 3, 1, 0],
			b: [1, 3, 1, 0],
			c: [2, 3, 1, 0],
			d: [1, 3, 2, 0]
		});
	});

	it('returns events in drawing order, so a stacked event comes after what it covers', () => {
		const result = layout(ev('late', '10:00', '11:00'), ev('long', '09:00', '12:00'));
		expect(result.map((p) => [p.id, p.indent])).toEqual([
			['long', 0],
			['late', 1]
		]);
	});

	// ---- what the view needs to tell overlapping events apart (design.md §11) -------------

	describe('touching', () => {
		const touching = (placements: Placement[]) =>
			Object.fromEntries(placements.map((p) => [p.id, p.touching]));

		it('marks events that overlap another one', () => {
			expect(
				touching(
					layout(ev('a', '09:00', '10:00'), ev('b', '09:00', '10:00'), ev('c', '11:00', '12:00'))
				)
			).toEqual({ a: true, b: true, c: false });
		});

		it('does not mark events that only meet end to start', () => {
			expect(touching(layout(ev('a', '09:00', '10:00'), ev('b', '10:00', '11:00')))).toEqual({
				a: false,
				b: false
			});
		});
	});

	describe('raised', () => {
		const raised = (placements: Placement[]) =>
			Object.fromEntries(placements.map((p) => [p.id, p.raised]));

		it('marks an event that starts later than one it overlaps', () => {
			expect(raised(layout(ev('a', '09:00', '11:00'), ev('b', '09:30', '10:30')))).toEqual({
				a: false,
				b: true
			});
		});

		it('does not mark events that start together', () => {
			expect(raised(layout(ev('a', '09:00', '10:00'), ev('b', '09:00', '11:00')))).toEqual({
				a: false,
				b: false
			});
		});

		it('marks a later event in a column of its own too', () => {
			// b starts 10 minutes after a, so it goes beside it, but it still starts later
			expect(
				raised(
					layout(ev('a', '09:00', '10:00'), ev('b', '09:10', '11:00'), ev('c', '10:00', '12:00'))
				)
			).toEqual({ a: false, b: true, c: true });
		});
	});
});
