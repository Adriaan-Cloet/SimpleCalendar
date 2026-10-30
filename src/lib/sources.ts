// Sources (ICS subscriptions, Google and CalDAV accounts) through the Tauri commands of
// docs/architecture.md §10. The shapes mirror the Rust types in `simplecalendar_core::service`.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type SourceKind = 'local' | 'ics' | 'google' | 'caldav';
export type SyncState = 'syncing' | 'ok' | 'error' | 'offline' | 'auth_required';

export interface SourceInfo {
	id: string;
	kind: SourceKind;
	display_name: string;
	/** The feed's host or the account; never the feed URL */
	account_label: string | null;
	/** Outcome of the last sync; `sync:status` events report a sync in progress */
	state: Exclude<SyncState, 'syncing'>;
	last_error: string | null;
	/** The user's own interval in seconds; `null` for the default */
	sync_interval_s: number | null;
	/** The default interval of the kind, in seconds */
	default_interval_s: number | null;
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
	| {
			kind:
				| 'invalid_input'
				| 'not_found'
				| 'keychain'
				| 'database'
				| 'google_client_missing'
				| 'sign_in'
				| 'cancelled'
				| 'not_editable'
				| 'unknown';
			message: string;
	  };

/** Which OAuth client Google sign-in uses; never holds the secret */
export interface GoogleClientInfo {
	/** The user's own client id. It takes precedence over the built-in one. */
	custom_client_id: string | null;
	/** Whether this build has a client compiled in */
	built_in: boolean;
}

export const getStatus = () => invoke<Status>('get_status');
export const listSources = () => invoke<SourceInfo[]>('list_sources');
export const addIcsSource = (url: string, name: string) =>
	invoke<SourceInfo>('add_ics_source', { url, name });
/** Resolves once the server accepted the login; rejects with `invalid_input` and a message for
 * the form if it did not. The password goes to the keychain only. */
export const addCaldavSource = (url: string, username: string, password: string) =>
	invoke<SourceInfo>('add_caldav_source', { url, username, password });
/** Checked against the server like `addCaldavSource`; the source syncs at once */
export const updateCaldavPassword = (source: string, password: string) =>
	invoke<SourceInfo>('update_caldav_password', { source, password });
export const removeSource = (source: string) => invoke<void>('remove_source', { source });
export const syncNow = (source: string | null = null) => invoke<void>('sync_now', { source });
/** From 5 minutes to a day; `null` goes back to the default. The source syncs at once. */
export const setSyncInterval = (source: string, seconds: number | null) =>
	invoke<SourceInfo>('set_sync_interval', { source, seconds });

/** Signs in in the system browser and resolves when that finishes. With `source`, signs that
 * account in again. */
export const beginGoogleSignIn = (source: string | null = null) =>
	invoke<SourceInfo>('begin_google_sign_in', { source });
export const cancelGoogleSignIn = () => invoke<void>('cancel_google_sign_in');
export const getGoogleClient = () => invoke<GoogleClientInfo>('get_google_client');
/** An empty `clientId` goes back to the built-in client */
export const setGoogleClient = (clientId: string, clientSecret: string) =>
	invoke<GoogleClientInfo>('set_google_client', { clientId, clientSecret });

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
