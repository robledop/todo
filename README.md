# Outlook Tasks - COSMIC applet

View, complete, and add Microsoft To Do tasks on a personal outlook.com account,
from the COSMIC panel.

## Runtime requirement

A Secret Service provider (gnome-keyring or KWallet) must be running - COSMIC
ships none by default. Without one, the applet shows a "No keyring found" notice
and sign-in is disabled until you install/start one.

## One-time app registration (Microsoft Entra)

The applet ships with an embedded public client id. To build your own:

1. Microsoft Entra admin center -> App registrations -> New registration.
2. Supported account types: **Personal Microsoft accounts only**.
3. Add a platform -> **Mobile and desktop applications** -> redirect URI
   `http://localhost`.
4. Authentication -> **Allow public client flows** -> Yes. (No client secret.)
5. API permissions -> Microsoft Graph -> Delegated -> add `Tasks.ReadWrite`,
   `offline_access`, `openid`.
6. Copy the Application (client) ID.

> Loopback note: the applet advertises `http://localhost:<port>/` and listens on
> `127.0.0.1`. This works wherever `localhost` resolves to `127.0.0.1` (the norm).
> On a host whose resolver prefers IPv6 `::1`, register `http://127.0.0.1` instead
> (added via the app manifest's `replyUrlsWithType`, since the portal text box
> rejects an http-scheme `127.0.0.1`) and the listener will still match.

## Build & install

```bash
OUTLOOK_TASKS_CLIENT_ID=<your-client-id> just build-release
sudo just install
```

Then add "Outlook Tasks" to the panel via COSMIC Settings -> Panel/Dock -> Applets.

## Scope (v1)

View tasks in any list, switch lists, complete tasks, add tasks. Background poll
every 5 minutes with an open-task count in the popup. Not yet: delete, edit,
due dates, reminders, subtasks, work/school accounts.
