# Google verification (#29)

The released app asks for two sensitive Google scopes. Until Google verifies the consent screen,
users see an "unverified app" warning, at most 100 accounts can sign in, and a project in
"Testing" gets refresh tokens that expire after 7 days (architecture.md §8, §13 point 8). This
document lists what to submit and the texts to paste. For a development client, see
[google-cloud.md](google-cloud.md).

Both scopes are **sensitive**, not restricted, so verification needs a demo video and
justifications but no third-party security assessment (CASA). Google's review usually takes a
few days to a few weeks, and the reviewer replies by e-mail to the developer contact.

## What is where

| What                         | Where                                                                                |
| ---------------------------- | ------------------------------------------------------------------------------------ |
| Homepage                     | <https://adriaan-cloet.github.io/SimpleCalendar/> (`site/index.html`)                |
| Privacy policy               | <https://adriaan-cloet.github.io/SimpleCalendar/privacy.html> (`site/privacy.html`)  |
| Authorized domain            | `adriaan-cloet.github.io`, verified in Search Console through the root user site     |
| Publishing                   | `.github/workflows/pages.yml`, on every push to `main` that changes `site/`          |
| Root user site               | Repository `Adriaan-Cloet/Adriaan-Cloet.github.io`, which holds the verification tag |
| Support and developer e-mail | cloetadriaan@gmail.com                                                               |
| Scopes                       | `google/oauth.rs` `SCOPES`; a change there needs a new verification                  |

`github.io` is on the Public Suffix List, so `adriaan-cloet.github.io` counts as a domain of
its own. Search Console verifies it by a tag at its root, which a project site cannot serve;
the user site provides that root, and the project site lives under the same domain.

## Steps

1. **Publish the site.** In this repository: **Settings → Pages → Source: GitHub Actions**.
   Merge to `main`; the "Site" workflow deploys `site/`. Both URLs above must open without a
   login.
2. **Verify the domain.** In [Search Console](https://search.google.com/search-console), add a
   **URL prefix** property `https://adriaan-cloet.github.io/`, choose **HTML tag**, and put the
   `<meta name="google-site-verification" …>` tag in the `<head>` of `index.html` in the
   `Adriaan-Cloet.github.io` repository. Push, wait for Pages, then press **Verify**. Use the
   same Google account that owns the Cloud project.
3. **Create the production project.** A Cloud project of its own, for example
   "SimpleCalendar", separate from the development one: its client is compiled into releases.
   Enable the **Google Calendar API**.
4. **Branding** (Google Auth Platform → Branding):
   - App name: `SimpleCalendar`
   - User support e-mail and developer contact: cloetadriaan@gmail.com
   - App logo: leave empty. The app's icon (`src-tauri/icons/icon.svg`, also `site/logo.svg`)
     is a placeholder until the branding after v1. A logo is optional, and each new one needs
     Google's approval again, so it is added once, with the final branding.
   - Application home page and privacy policy link: the two URLs above.
   - Authorized domain: `adriaan-cloet.github.io`.
5. **Audience:** External.
6. **Data access:** add exactly the two scopes below, with the justifications.
7. **Clients → Create client:** type **Desktop app**, name "SimpleCalendar". Store the id and
   secret as the repository secrets `SIMPLECALENDAR_GOOGLE_CLIENT_ID` and
   `SIMPLECALENDAR_GOOGLE_CLIENT_SECRET`; the release workflow (#32) passes them to the build
   (`src-tauri/build.rs`). For the demo video, put them in the local `.env` and
   `touch src-tauri/build.rs`.
8. **Record the demo video** (script below) and upload it to YouTube as **Unlisted**.
9. **Audience → Publish app**, then **Verification Center → Prepare for verification**: fill in
   the justifications and the video link, and submit.
10. **After approval:** sign in with a Google account that is not a test user. More than 7 days
    later, check that it still syncs without "Sign in again". Then record the result in
    architecture.md §13 point 8 and tick the last boxes of #29.

## Scope justifications

Google asks for one text per scope and one for the app as a whole. The limit is 1000
characters each.

**How the app uses Google user data:**

> SimpleCalendar is an open-source desktop calendar for Ubuntu and Windows. It shows the
> user's Google calendars next to their other calendars in a day, week and month view, and
> lets the user create, edit and delete events. All data is stored only on the user's
> computer; tokens are kept in the operating system's keychain. The app has no server: it
> talks only to Google's Calendar API, and no data is sent to the developer or any third
> party.

**`https://www.googleapis.com/auth/calendar.calendarlist.readonly`:**

> Used to list the calendars the user can see, with their names, colours, time zones and
> access level, so the app can show each calendar in its sidebar, colour its events and know
> whether the user may edit it. The app never changes the calendar list, so the read-only
> scope is enough. It also reads the primary calendar's id, which is the account's address,
> to label the account; that is why no email or profile scope is requested.

**`https://www.googleapis.com/auth/calendar.events`:**

> Used to read the events of the user's calendars, so the app can show them, and to create,
> update and delete events when the user does so in the app's event editor. This includes
> calendars shared with the user where they have write access, which is why
> calendar.events.owned is not enough. The broader calendar scope is not requested because
> the app never creates, shares or deletes calendars.

## Demo video script

Google wants to see the whole OAuth flow and every scope in use, in English, with the OAuth
client id visible in the browser's address bar. Three to five minutes is enough. Record the
screen with sound off or with a short spoken commentary; zoom in where text is small.

1. Show the homepage, scroll to the link to the privacy policy, and open it.
2. Start SimpleCalendar, built with the production client. Open **Accounts** and, under "Add a
   Google account", press **Sign in with Google**.
3. The browser opens Google's consent page. Click the address bar and scroll through the URL
   until `client_id=…` is visible; it must match the client in the submission.
4. Choose the account. Show the consent screen with the app name "SimpleCalendar" and both
   permissions, and allow them.
5. Back in the app: the account appears in Accounts, and its calendars appear in the sidebar
   (**calendarlist.readonly**). Show a calendar's colour and hide and show one.
6. Show the week view with the Google events (**calendar.events**, read).
7. Create an event in a Google calendar, then open Google Calendar on the web and show that it
   arrived. Change its title and time in the app and show the change on the web. Delete it and
   show it is gone (**calendar.events**, write).
8. Open **Accounts**, remove the Google account, and show that its calendars are gone from the
   app. Finish on the Google account's **Third-party apps and services** page, where access can
   be withdrawn.

## Keeping it verified

- A new scope, a new app name or logo, or a new homepage or privacy URL needs a new review.
- Keep `site/privacy.html` in line with what the app does; a reviewer may compare the two. In
  particular, update it if the app starts sending data anywhere new (for example location
  search, #51, or AI access, architecture.md §14).
