// Sources (ICS subscriptions, later Google and CalDAV accounts) through the Tauri commands of
// docs/architecture.md §10. The shapes mirror the Rust types in `simplecalendar_core::service`.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type SourceKind = 'local' | 'ics' | 'google' | 'caldav';
export type SyncState = 'syncing' | 'ok' | 'error' | 'auth_required';

export interface SourceInfo {
	id: string;
	kind: SourceKind;
	display_name: string;
	/** The feed's host or the account; never the feed URL */
	account_label: string | null;
	/** Outcome of the last sync; `sync:status` events report a sync in progress */
	state: Exclude<SyncState, 'syncing'>;
	last_error: string | null;
	/** Epoch seconds */
	last_synced_at: number | null;
	warnings: string[];
}

/** Payload of `sync:status` */
export interface SyncStatus {
	source_id: string;
	state: SyncState;
	last_error: string | null;
}

export interface SecretsUnavailable {
	kind: 'secrets_unavailable';
	reason: 'no_service' | 'locked';
	message: string;
}

export interface Status {
	secrets_unavailable: SecretsUnavailable | null;
}

/** What a failed command rejects with */
export type ServiceError =
	| SecretsUnavailable
	| { kind: 'invalid_input' | 'not_found' | 'keychain' | 'database' | 'unknown'; message: string };

export const getStatus = () => invoke<Status>('get_status');
export const listSources = () => invoke<SourceInfo[]>('list_sources');
export const addIcsSource = (url: string, name: string) =>
	invoke<SourceInfo>('add_ics_source', { url, name });
export const removeSource = (source: string) => invoke<void>('remove_source', { source });
export const syncNow = (source: string | null = null) => invoke<void>('sync_now', { source });

export const onSyncStatus = (handler: (status: SyncStatus) => void): Promise<UnlistenFn> =>
	listen<SyncStatus>('sync:status', (event) => handler(event.payload));

/** A rejected command as a `ServiceError`. Anything else, such as running in a plain browser
 * without Tauri, becomes `unknown`. */
export function serviceError(e: unknown): ServiceError {
	if (typeof e === 'object' && e !== null && 'kind' in e && 'message' in e) {
		return e as ServiceError;
	}
	return { kind: 'unknown', message: 'Something went wrong. Try again.' };
}
