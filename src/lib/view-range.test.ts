import { describe, expect, it } from 'vitest';

import { inRange, ownRange, step, viewRange, viewTitle } from './view-range.js';

// Friday 23 October 2026
const friday = new Date(2026, 9, 23);

describe('viewRange', () => {
	it('is the Monday-first week around the date in the week view', () => {
		expect(viewRange('week', friday)).toEqual({ start: new Date(2026, 9, 19), days: 7 });
	});

	it('is the date itself in the day view', () => {
		expect(viewRange('day', new Date(2026, 9, 23, 15, 30))).toEqual({ start: friday, days: 1 });
	});

	it('is the Monday-first weeks that cover the month in the month view', () => {
		// October 2026 runs from Thursday 1 to Saturday 31: five weeks from Monday 28 September
		expect(viewRange('month', friday)).toEqual({ start: new Date(2026, 8, 28), days: 35 });
		// August 2026 starts on a Saturday and ends on a Monday: six weeks
		expect(viewRange('month', new Date(2026, 7, 15))).toEqual({
			start: new Date(2026, 6, 27),
			days: 42
		});
		// February 2027 starts on a Monday and has exactly four weeks
		expect(viewRange('month', new Date(2027, 1, 10))).toEqual({
			start: new Date(2027, 1, 1),
			days: 28
		});
	});
});

describe('ownRange', () => {
	it('is only the days of the month in the month view', () => {
		expect(ownRange('month', friday)).toEqual({ start: new Date(2026, 9, 1), days: 31 });
		expect(ownRange('month', new Date(2028, 1, 29))).toEqual({
			start: new Date(2028, 1, 1),
			days: 29
		});
	});

	it('is the shown days in the week and day views', () => {
		expect(ownRange('week', friday)).toEqual(viewRange('week', friday));
		expect(ownRange('day', friday)).toEqual(viewRange('day', friday));
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

	it('moves the date a month in the month view, keeping its day where the month has it', () => {
		expect(step('month', friday, 1)).toEqual(new Date(2026, 10, 23));
		expect(step('month', friday, -1)).toEqual(new Date(2026, 8, 23));
		expect(step('month', new Date(2026, 11, 15), 1)).toEqual(new Date(2027, 0, 15));
	});

	it('moves to the last day of a shorter month', () => {
		expect(step('month', new Date(2026, 9, 31), 1)).toEqual(new Date(2026, 10, 30));
		expect(step('month', new Date(2027, 2, 31), -1)).toEqual(new Date(2027, 1, 28));
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

	it('names the month in the month view, also when its weeks reach into others', () => {
		expect(viewTitle('month', friday, 'en-GB')).toBe('October 2026');
		expect(viewTitle('month', new Date(2026, 7, 1), 'en-GB')).toBe('August 2026');
	});

	it('names the day in the day view', () => {
		expect(viewTitle('day', friday, 'en-GB')).toMatch(/^Friday,? 23 October 2026$/);
	});
});
