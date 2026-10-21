import { describe, expect, it } from 'vitest';
import {
	displayLocale,
	formatDateField,
	parseDateField,
	parseTimeField,
	timeOptions,
	durationLabel
} from './display-format.js';

describe('displayLocale', () => {
	it('writes the day first and a 24-hour clock, whatever the system says', () => {
		const date = new Date(2026, 9, 20, 21, 5);
		expect(new Intl.DateTimeFormat(displayLocale, { dateStyle: 'short' }).format(date)).toBe(
			'20/10/2026'
		);
		expect(
			new Intl.DateTimeFormat(displayLocale, { hour: 'numeric', minute: '2-digit' }).format(date)
		).toBe('21:05');
	});
});

describe('date field', () => {
	it('shows a date day first', () => {
		expect(formatDateField('2026-10-05')).toBe('05/10/2026');
	});

	it('reads what people type', () => {
		for (const text of [
			'20/10/2026',
			'20-10-2026',
			'20.10.2026',
			'20 10 2026',
			'20/10/26',
			' 20/10/2026 '
		]) {
			expect(parseDateField(text, 2026), text).toBe('2026-10-20');
		}
		expect(parseDateField('5/1/2027', 2026)).toBe('2027-01-05');
		expect(parseDateField('2026-10-20', 2026)).toBe('2026-10-20');
	});

	it('takes the given year when it is left out', () => {
		expect(parseDateField('20/10', 2027)).toBe('2027-10-20');
	});

	it('refuses what is not a date', () => {
		for (const text of [
			'',
			'31/02/2026',
			'10/20/2026',
			'20/13/2026',
			'tomorrow',
			'2026',
			'20/10/2026x'
		]) {
			expect(parseDateField(text, 2026), text).toBeNull();
		}
	});
});

describe('time field', () => {
	it('reads what people type', () => {
		const cases: [string, string][] = [
			['9:30', '09:30'],
			['09:30', '09:30'],
			['21.30', '21:30'],
			['21h30', '21:30'],
			['0930', '09:30'],
			['930', '09:30'],
			['9', '09:00'],
			['17', '17:00'],
			['0:00', '00:00'],
			[' 23:59 ', '23:59']
		];
		for (const [text, value] of cases) expect(parseTimeField(text), text).toBe(value);
	});

	it('refuses what is not a time', () => {
		for (const text of ['', '24:00', '9:60', '9 AM', 'noon', '12345', '9:3']) {
			expect(parseTimeField(text), text).toBeNull();
		}
	});
});

describe('timeOptions', () => {
	it('lists the day in quarters of an hour', () => {
		const all = timeOptions(null);
		expect(all).toHaveLength(96);
		expect(all.slice(0, 3)).toEqual(['00:00', '00:15', '00:30']);
		expect(all.at(-1)).toBe('23:45');
	});

	it('starts at a given time and keeps its minutes, until the end of the day', () => {
		expect(timeOptions('22:10')).toEqual([
			'22:10',
			'22:25',
			'22:40',
			'22:55',
			'23:10',
			'23:25',
			'23:40',
			'23:55'
		]);
	});
});

describe('durationLabel', () => {
	it('says how long, in minutes and hours', () => {
		expect(durationLabel(0)).toBe('0 min');
		expect(durationLabel(15)).toBe('15 min');
		expect(durationLabel(60)).toBe('1 h');
		expect(durationLabel(90)).toBe('1 h 30 min');
		expect(durationLabel(615)).toBe('10 h 15 min');
	});
});
