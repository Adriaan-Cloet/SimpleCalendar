import { describe, expect, it } from 'vitest';
import { addDays, isoDate, startOfWeek } from './week-dates.js';

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
