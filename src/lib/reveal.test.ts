import { describe, expect, it } from 'vitest';

import { revealScrollTop, type RevealBox } from './reveal.js';

// A day view scrolled to 07:30 with 48px hours: 600px tall, a 100px header on top
const view = (top: number, bottom: number): RevealBox => ({
	scrollTop: 360,
	viewHeight: 600,
	covered: 100,
	top,
	bottom,
	margin: 48
});

describe('revealScrollTop', () => {
	it('leaves an event that shows in full where it is', () => {
		expect(revealScrollTop(view(480, 528))).toBeNull();
		// Exactly at both edges of what shows
		expect(revealScrollTop(view(460, 960))).toBeNull();
	});

	it('scrolls an event further down the day to an hour below the header', () => {
		// 21:00 to 22:00
		expect(revealScrollTop(view(1008, 1056))).toBe(1008 - 100 - 48);
	});

	it('scrolls back up to an early event, or to the very top', () => {
		// 06:00, half hidden under the header
		expect(revealScrollTop(view(288, 336))).toBe(140);
		// 00:30
		expect(revealScrollTop(view(24, 72))).toBe(0);
	});

	it('shows the start of an event taller than the view', () => {
		expect(revealScrollTop(view(400, 1400))).toBe(400 - 100 - 48);
	});
});
