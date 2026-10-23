import { describe, expect, it } from 'vitest';

import {
	conflictNotice,
	issueCountLabel,
	issueReason,
	issueSummary,
	issueTime
} from './sync-issue-text.js';

describe('issueCountLabel', () => {
	it('counts changes in the singular and plural', () => {
		expect(issueCountLabel(1)).toBe('1 change not saved');
		expect(issueCountLabel(3)).toBe('3 changes not saved');
	});
});

describe('issueSummary', () => {
	it('says what the change was', () => {
		expect(issueSummary({ change: 'create', title: 'Lunch' })).toBe('New event “Lunch”');
		expect(issueSummary({ change: 'update', title: 'Lunch' })).toBe('Change to “Lunch”');
		expect(issueSummary({ change: 'delete', title: 'Lunch' })).toBe('Deleting “Lunch”');
	});

	it('names an event without a title', () => {
		expect(issueSummary({ change: 'update', title: '  ' })).toBe('Change to “Untitled event”');
	});
});

describe('issueReason', () => {
	it('starts the reason with a capital', () => {
		expect(issueReason({ kind: 'failed', reason: 'the server refused: forbidden' })).toBe(
			'The server refused: forbidden'
		);
	});

	it('falls back to a reason per kind', () => {
		expect(issueReason({ kind: 'conflicted', reason: '' })).toBe(
			'The event was changed elsewhere, so this change was not saved.'
		);
		expect(issueReason({ kind: 'failed', reason: '' })).toBe(
			'The server did not accept this change.'
		);
	});
});

describe('issueTime', () => {
	it('writes the day first and a 24-hour clock', () => {
		// 23 October 2026, 14:05 UTC; the test reads it back in the same zone
		const at = Date.UTC(2026, 9, 23, 14, 5) / 1000;
		const local = new Date(at * 1000);
		const expected = `${local.getDate()} Oct, ${String(local.getHours()).padStart(2, '0')}:05`;
		expect(issueTime(at)).toBe(expected);
	});
});

describe('conflictNotice', () => {
	it('names the event of a single conflict', () => {
		expect(conflictNotice(['Lunch'])).toEqual({
			title: 'Your change to “Lunch” wasn’t saved',
			detail: 'The event was changed elsewhere, so that version was kept.'
		});
	});

	it('counts several conflicts', () => {
		expect(conflictNotice(['Lunch', 'Gym']).title).toBe('2 changes weren’t saved');
	});
});
