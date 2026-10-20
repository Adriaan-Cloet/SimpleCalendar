import { describe, expect, it } from 'vitest';
import { calendarColor } from './calendar-color.js';

describe('calendarColor', () => {
	it('maps a palette hex to its token, in any case', () => {
		expect(calendarColor('#007AFF', 1)).toBe('var(--cal-blue)');
	});

	it('uses any other hex as is', () => {
		expect(calendarColor('#e67c73', 1)).toBe('#e67c73');
	});

	it('picks a palette colour by calendar id when there is none', () => {
		expect(calendarColor(null, 1)).toBe('var(--cal-blue)');
		expect(calendarColor(null, 2)).toBe('var(--cal-orange)');
		expect(calendarColor(null, 11)).toBe('var(--cal-blue)');
	});
});
