import { describe, expect, it } from 'vitest';
import { addDays, isoDate, monthWeeks, startOfWeek } from './week-dates.js';

describe('week dates', () => {
	it('starts a week on Monday', () => {
		expect(isoDate(startOfWeek(new Date(2026, 9, 25, 23, 30)))).toBe('2026-10-19');
		expect(isoDate(startOfWeek(new Date(2026, 9, 19)))).toBe('2026-10-19');
	});

	it('steps over month and year ends', () => {
		expect(isoDate(addDays(new Date(2026, 11, 28), 7))).toBe('2027-01-04');
		expect(isoDate(addDays(new Date(2026, 0, 4), -7))).toBe('2025-12-28');
	});

	it('pads months and days', () => {
		expect(isoDate(new Date(2026, 0, 5))).toBe('2026-01-05');
	});
});

describe('monthWeeks', () => {
	const summary = (weeks: Date[][]) => weeks.map((w) => `${isoDate(w[0])}..${isoDate(w[6])}`);

	it('covers the month in whole weeks, Monday first', () => {
		// October 2026 starts on a Thursday and ends on a Saturday
		expect(summary(monthWeeks(2026, 9))).toEqual([
			'2026-09-28..2026-10-04',
			'2026-10-05..2026-10-11',
			'2026-10-12..2026-10-18',
			'2026-10-19..2026-10-25',
			'2026-10-26..2026-11-01'
		]);
	});

	it('can always give six weeks, so a picker keeps its height', () => {
		const weeks = monthWeeks(2026, 1, 6);
		// February 2026 starts on a Sunday
		expect(weeks).toHaveLength(6);
		expect(isoDate(weeks[0][0])).toBe('2026-01-26');
		expect(isoDate(weeks[5][6])).toBe('2026-03-08');
	});
});
