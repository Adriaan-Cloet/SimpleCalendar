import { describe, expect, it } from 'vitest';

import { intervalChoices, intervalLabel, summarize } from './sync-summary.js';

describe('summarize', () => {
	it('shows nothing without sources', () => {
		expect(summarize({}).kind).toBe('none');
	});

	it('is up to date when every source is', () => {
		const summary = summarize({ a: 'ok', b: 'ok' });
		expect(summary.kind).toBe('ok');
		expect(summary.label).toBeNull();
		expect(summary.opensAccounts).toBe(false);
	});

	it('shows a sync in progress over anything else', () => {
		expect(summarize({ a: 'auth_required', b: 'syncing', c: 'error' }).kind).toBe('syncing');
	});

	it('asks to sign in again before reporting a failure', () => {
		const summary = summarize({ a: 'error', b: 'auth_required' });
		expect(summary.kind).toBe('auth_required');
		expect(summary.label).toBe('Sign in again');
		expect(summary.opensAccounts).toBe(true);
	});

	it('is offline only when no source could connect', () => {
		const summary = summarize({ a: 'offline', b: 'offline' });
		expect(summary.kind).toBe('offline');
		expect(summary.label).toBe('Offline');
		// Clicking tries again rather than opening the dialog
		expect(summary.opensAccounts).toBe(false);
	});

	it('treats one unreachable server among working ones as a failure', () => {
		expect(summarize({ a: 'offline', b: 'ok' }).kind).toBe('error');
	});

	it('reports a failure that opens Accounts', () => {
		const summary = summarize({ a: 'ok', b: 'error' });
		expect(summary.kind).toBe('error');
		expect(summary.label).toBe('Sync failed');
		expect(summary.opensAccounts).toBe(true);
	});
});

describe('intervalLabel', () => {
	it('names minutes, hours and a day', () => {
		expect(intervalLabel(300)).toBe('Every 5 minutes');
		expect(intervalLabel(3600)).toBe('Every hour');
		expect(intervalLabel(6 * 3600)).toBe('Every 6 hours');
		expect(intervalLabel(86_400)).toBe('Every day');
	});
});

describe('intervalChoices', () => {
	it('puts the default first and leaves it out of the rest', () => {
		const choices = intervalChoices(900, null);
		expect(choices[0]).toEqual({ value: null, label: 'Every 15 minutes (default)' });
		expect(choices.map((c) => c.value)).not.toContain(900);
		expect(choices.map((c) => c.value)).toContain(300);
	});

	it('offers a stored value that is not one of the choices, in order', () => {
		const values = intervalChoices(3600, 600).map((c) => c.value);
		expect(values.slice(0, 4)).toEqual([null, 300, 600, 900]);
	});
});
