// What the toolbar's sync indicator shows (issue #22, docs/architecture.md §7.1), and the
// interval choices of the Accounts dialog. Pure, so it is tested without a DOM.
import type { SyncState } from './sources.js';

export type SummaryKind = 'none' | 'ok' | 'syncing' | 'offline' | 'error' | 'auth_required';

export interface SyncSummary {
	kind: SummaryKind;
	/** Shown next to the icon; `null` when the icon says enough */
	label: string | null;
	/** Tooltip and accessible name */
	title: string;
	/** Whether a click opens the Accounts dialog instead of syncing again */
	opensAccounts: boolean;
}

/** One summary for the state of every remote source, by source id. A sync in progress wins;
 * then a source that needs signing in, then a failure. Offline only shows when no source
 * could connect: one unreachable server among working ones is a failure of that source. */
export function summarize(states: Record<string, SyncState>): SyncSummary {
	const all = Object.values(states);
	if (all.length === 0) {
		return { kind: 'none', label: null, title: '', opensAccounts: false };
	}
	if (all.includes('syncing')) {
		return { kind: 'syncing', label: null, title: 'Syncing…', opensAccounts: false };
	}
	if (all.includes('auth_required')) {
		return {
			kind: 'auth_required',
			label: 'Sign in again',
			title: 'An account needs you to sign in again. Open Accounts.',
			opensAccounts: true
		};
	}
	if (all.every((state) => state === 'offline')) {
		return {
			kind: 'offline',
			label: 'Offline',
			title: 'No connection. Changes are kept and sent once you are back online. Click to try now.',
			opensAccounts: false
		};
	}
	if (all.some((state) => state === 'error' || state === 'offline')) {
		return {
			kind: 'error',
			label: 'Sync failed',
			title: 'A calendar could not be synced. Open Accounts for details.',
			opensAccounts: true
		};
	}
	return { kind: 'ok', label: null, title: 'Up to date. Click to sync now.', opensAccounts: false };
}

/** The intervals the Accounts dialog offers, in seconds */
export const SYNC_INTERVALS = [300, 900, 1800, 3600, 3 * 3600, 6 * 3600, 12 * 3600, 86_400];

/** "Every 15 minutes", "Every hour", "Every 6 hours", "Every day" */
export function intervalLabel(seconds: number): string {
	const minutes = Math.round(seconds / 60);
	if (minutes < 60) return `Every ${minutes} minutes`;
	const hours = Math.round(minutes / 60);
	if (hours === 1) return 'Every hour';
	if (hours < 24) return `Every ${hours} hours`;
	return 'Every day';
}

/** The choices for one source: its default first, then the others. A stored value that is not
 * one of them (set by an older version) is offered too, so the select can show it. */
export function intervalChoices(
	defaultSeconds: number,
	current: number | null
): { value: number | null; label: string }[] {
	const values = SYNC_INTERVALS.filter((s) => s !== defaultSeconds);
	if (current !== null && current !== defaultSeconds && !values.includes(current)) {
		values.push(current);
		values.sort((a, b) => a - b);
	}
	return [
		{ value: null, label: `${intervalLabel(defaultSeconds)} (default)` },
		...values.map((value) => ({ value, label: intervalLabel(value) }))
	];
}
