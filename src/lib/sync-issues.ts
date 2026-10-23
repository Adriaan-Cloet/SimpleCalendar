// Changes the server did not save (issue #21, docs/architecture.md §7.3, §10): the sync issues
// list and the `sync:conflict` notice. The shapes mirror `simplecalendar_core::service`.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface SyncIssue {
	/** The outbox row, which `sync:conflict` names too */
	id: number;
	/** `conflicted`: the event changed elsewhere and that version was kept. `failed`: the
	 * server refused the change and it was undone. */
	kind: 'conflicted' | 'failed';
	change: 'create' | 'update' | 'delete';
	/** The title in the user's change; for a delete, the title the event had */
	title: string;
	reason: string;
	calendar_id: number;
	calendar_name: string;
	/** Epoch seconds */
	created_at: number;
	/** When false, only dismissing is left */
	can_redo: boolean;
}

/** Payload of `sync:conflict` */
export interface SyncConflict {
	outbox_id: number;
	title: string;
}

export const listSyncIssues = () => invoke<SyncIssue[]>('list_sync_issues');
export const dismissSyncIssue = (id: number) => invoke<void>('dismiss_sync_issue', { id });
export const redoSyncIssue = (id: number) => invoke<void>('redo_sync_issue', { id });

export const onSyncConflict = (handler: (conflict: SyncConflict) => void): Promise<UnlistenFn> =>
	listen<SyncConflict>('sync:conflict', (event) => handler(event.payload));
