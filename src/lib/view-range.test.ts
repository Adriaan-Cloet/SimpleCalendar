import { describe, expect, it } from 'vitest';

import { inRange, step, viewRange, viewTitle } from './view-range.js';

// Friday 23 October 2026
const friday = new Date(2026, 9, 23);

describe('viewRange', () => {
	it('is the Monday-first week around the date in the week view', () => {
		expect(viewRange('week', friday)).toEqual({ start: new Date(2026, 9, 19), days: 7 });
	});

	it('is the date itself in the day view', () => {
		expect(viewRange('day', new Date(2026, 9, 23, 15, 30))).toEqual({ start: friday, days: 1 });
	});
});

describe('step', () => {
	it('moves the date a week in the week view, keeping its weekday', () => {
		expect(step('week', friday, 1)).toEqual(new Date(2026, 9, 30));
		expect(step('week', friday, -1)).toEqual(new Date(2026, 9, 16));
	});

	it('moves the date a day in the day view, across months', () => {
		expect(step('day', new Date(2026, 9, 31), 1)).toEqual(new Date(2026, 10, 1));
		expect(step('day', friday, -1)).toEqual(new Date(2026, 9, 22));
	});

	it('keeps whole days across a change of summer time', () => {
		// Europe drops an hour on 25 October 2026; the next day still starts at midnight
		expect(step('day', new Date(2026, 9, 25), 1)).toEqual(new Date(2026, 9, 26));
	});
});

describe('inRange', () => {
	it('tells whether a moment falls on one of the shown days', () => {
		const week = viewRange('week', friday);
		expect(inRange(week, new Date(2026, 9, 25, 23, 59))).toBe(true);
		expect(inRange(week, new Date(2026, 9, 26))).toBe(false);
		expect(inRange(viewRange('day', friday), new Date(2026, 9, 22, 23))).toBe(false);
	});
});

describe('viewTitle', () => {
	it('names the month or months of the week', () => {
		expect(viewTitle('week', friday, 'en-GB')).toBe('October 2026');
		// The exact punctuation depends on the webview's ICU version
		const both = viewTitle('week', new Date(2026, 9, 28), 'en-GB');
		expect(both).toMatch(/October.*November 2026/);
	});

	it('names the day in the day view', () => {
		expect(viewTitle('day', friday, 'en-GB')).toMatch(/^Friday,? 23 October 2026$/);
	});
});
