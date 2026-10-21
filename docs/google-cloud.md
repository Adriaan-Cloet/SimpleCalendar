# Google Cloud client for development

Google sign-in needs an OAuth client from a Google Cloud project. The released app will have one
compiled in (#29); for development you make your own. It takes about ten minutes. The console's
menu names change now and then; the steps below match the console as of October 2026.

## Create the client

1. Open <https://console.cloud.google.com/> and create a project, for example
   "SimpleCalendar dev".
2. **APIs & Services → Library**: search for "Google Calendar API" and enable it.
3. **Google Auth Platform → Branding**: enter an app name and your address as support and
   developer contact.
4. **Google Auth Platform → Audience**: choose **External** and leave the publishing status at
   **Testing**. Under **Test users**, add the Google accounts you will sign in with. Only those
   accounts can sign in while the app is in Testing.
5. **Google Auth Platform → Data access**: add the two scopes the app asks for:
   - `https://www.googleapis.com/auth/calendar.calendarlist.readonly`
   - `https://www.googleapis.com/auth/calendar.events`
6. **Google Auth Platform → Clients → Create client**: application type **Desktop app**. Copy
   the client ID and the client secret.

A Desktop client accepts any `http://127.0.0.1:<port>/` redirect, so there is no redirect URI
to register.

## Use it

Either build it in: copy `.env.example` to `.env` at the repository root, fill in both values,
run `touch src-tauri/build.rs` once, and start `npm run tauri dev`. `.env` is ignored by git.

Or enter it in the app: **Accounts → Use your own client…**. The id is stored in the database,
the secret in the system keyring. It takes precedence over a built-in client.

## What to expect in Testing

- Google shows a warning that the app is not verified. Continue past it; that is expected until
  verification (#29).
- Refresh tokens expire after 7 days. The account then shows "Sign in again to keep this
  account in sync." in the Accounts dialog (architecture.md §13 point 8).
