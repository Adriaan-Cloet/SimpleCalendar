// The words of the sync issues list and the conflict notice. Kept out of the components so
// they can be tested.
import { displayLocale } from './display-format.js';
import type { SyncIssue } from './sync-issues.js';

function quoted(title: string): string {
	return `“${title.trim() || 'Untitled event'}”`;
}

/** The sidebar's link to the list: "1 change not saved" */
export function issueCountLabel(count: number): string {
	return `${count} ${count === 1 ? 'change' : 'changes'} not saved`;
}

/** What the user tried to do: "Change to “Lunch”" */
export function issueSummary(issue: Pick<SyncIssue, 'change' | 'title'>): string {
	const title = quoted(issue.title);
	switch (issue.change) {
		case 'create':
			return `New event ${title}`;
		case 'update':
			return `Change to ${title}`;
		case 'delete':
			return `Deleting ${title}`;
	}
}

/** Why it was not saved, as a sentence. Provider errors start in lower case. */
export function issueReason(issue: Pick<SyncIssue, 'kind' | 'reason'>): string {
	const reason = issue.reason.trim();
	if (!reason) {
		return issue.kind === 'conflicted'
			? 'The event was changed elsewhere, so this change was not saved.'
			: 'The server did not accept this change.';
	}
	return reason.charAt(0).toUpperCase() + reason.slice(1);
}

/** When the change was made: "23 Oct, 11:10" */
export function issueTime(createdAt: number, locale = displayLocale): string {
	return new Intl.DateTimeFormat(locale, {
		day: 'numeric',
		month: 'short',
		hour: '2-digit',
		minute: '2-digit',
		hour12: false
	}).format(new Date(createdAt * 1000));
}

/** The notice after `sync:conflict`, for the titles of every conflict since it appeared */
export function conflictNotice(titles: string[]): { title: string; detail: string } {
	if (titles.length === 1) {
		return {
			title: `Your change to ${quoted(titles[0])} wasn’t saved`,
			detail: 'The event was changed elsewhere, so that version was kept.'
		};
	}
	return {
		title: `${titles.length} changes weren’t saved`,
		detail: 'The events were changed elsewhere, so those versions were kept.'
	};
}
