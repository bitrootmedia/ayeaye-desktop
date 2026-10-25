# ayeaye desktop

A native desktop client for [ayeaye](../ayeaye), in Rust on
[GPUI](https://github.com/zed-industries/zed) — Zed's own UI framework — through
[GPUI Kit](https://gpui-kit.com), with Zed's tabs, quick open and feel.

The rail at the left holds the modules. The organisation's:

- **Notes** — the notepad that replaces a folder of loose text files
- **Tasks** — Waiting on me, Mine, Triage, All open, by project; a task opens
  in a tab with its fields, description, checklists, reminders, comments and
  the timer
- **My planner** — Today, Tomorrow, This week, Next week, Someday
- **Knowledge base** — books and articles; read rendered, edit as markdown,
  publish

and the person's own, which ignore the organisation:

- **Sparks** — quick capture (⌘J from anywhere), turn one into a task
- **Reminders** — due and upcoming across every organisation, tick or snooze
- **Inbox** — notifications, with the unread count on the rail

Plus the running timer in the status bar (with the server's "still working on
this?" check-in), ⌘⇧F search across the organisation, and live updates: a
task changed or commented on elsewhere changes here as it happens (the green
dot at the right of the status bar).

Multi-profile like the [mobile app](../ayeaye-mobile): a profile is one server
URL plus one credential set, several can be signed in at once, and organisations
switch inside a profile.

---

## Running it

```sh
make run          # release build
make dev          # debug build, tokens in a file (see below)
make install      # dist/Ayeaye.app → ~/Applications
make test         # unit tests, no server needed
make live-test    # against a real instance (AYEAYE_TEST_SERVER/EMAIL/PASSWORD)
```

The toolchain is pinned in `rust-toolchain.toml` (Rust 1.99.1). rustup installs
it on first build **alongside** the machine's default, which is left alone.
GPUI compiles its Metal shaders at build time, so macOS needs full Xcode, not
just the command line tools. The first build takes ~7 minutes; after that,
seconds.

Linux and Windows are GPUI targets too and nothing here is macOS-only except
`script/bundle-macos.sh`; neither has been built yet.

### Pointing it at a local instance

```sh
cd ../ayeaye && docker compose up -d     # http://localhost
```

Type `http://localhost` as the server. `http://` is warned about, never blocked.

---

## Using it

| Keys | |
|---|---|
| ⌘N | New note — the tab opens instantly, the server catches up |
| ⌘⇧N | New task in this organisation, opened in a tab |
| ⌘J | Capture a spark |
| ⌘⇧F | Search the organisation |
| ⌘⌥1–7 | Notes, Tasks, Planner, Knowledge base, Sparks, Reminders, Inbox |
| ⌘P / ⌘K | Find a note; Return on no match creates one with that title |
| ⌘⇧E | Focus the sidebar's filter (in Notes, ↑/↓ preview as you go) |
| ⌘W, ⌘⇧T | Close tab, reopen the last closed one |
| ⌃Tab, ⌘⇧[ ⌘⇧], ⌘1–9 | Move between tabs (⌘9 is the last) |
| ⌘B | Toggle the sidebar |
| ⌘S | Save now (it autosaves anyway) |
| ⌘⇧⌫ | Delete the open note, after asking |
| ⌘R | Refetch what's showing, the inbox and the timer |
| ⌘⏎ | Send a comment |
| ⌘F | Find within the note |

On Linux and Windows, ⌘ is Ctrl.

Single-clicking a note opens it in an *italic preview tab* that the next click
replaces; typing in it, or double-clicking, keeps it — Zed's preview tabs. Tabs
drag to reorder, middle-click closes, and the tabs open in each organisation are
reopened next launch.

---

## How it is put together

```
src/
  api/          ApiClient (the SuperTokens header-mode lifecycle), endpoints, DTOs
  store/        profiles.json, the keychain vault, cached notes, open tabs
  session.rs    the profile's client and "me"; an organisation's projects/members
  timer.rs      the one running timer, polled and ticking
  ui/           root (sign-in ↔ gates ↔ workspace), workspace, pane + item,
                open (every "open this"), quick open, search, prompt, widgets
  notes/        Notes: store, sidebar panel, editor
  tasks/        Tasks: list panel, planner panel, task view
  kb/           Knowledge base: books panel, article view
  personal/     sparks, reminders, inbox
  theme.rs      Zed's One Dark / One Light (assets/themes/one.json)
  devtools.rs   scripted input + screenshots, `--features devtools` only
```

**Tabs belong to no module.** `ui/pane.rs` holds anything implementing
`ui/item.rs`'s `Item` — a title, an icon, a dirty flag, a status line, `flush`.
Notes, tasks, articles and sparks are all items in the same strip, and all
reopen on the next launch. Panels never create tabs themselves: they emit an
`Open` (`ui/open.rs`) and the workspace turns it into a tab — switching
organisation first when an inbox link or a reminder points at another one. A
new module is a rail entry in `ui/modules.rs`, a panel, and items.

**Tasks are never optimistic about shared work.** Every field change is one
request, and the server's answer replaces what's on screen; controls a `can_*`
flag or the access level (`read`/`write`/`owner`) denies are hidden. Only a
checklist tick shows before the server answers, and it is reverted if refused.

**Rich text: render HTML, edit markdown.** Descriptions, comments and articles
are sanitised HTML; they render with GPUI Kit's HTML view. Editing converts
the HTML to markdown (`htmd`) and sends it back with `*_format: markdown`, the
door the server built for exactly this. Headings, lists, bold, code and links
survive the round trip.

**Realtime is a doorbell, not a feed.** `WS /api/ws` events carry no content
— "task X moved", "a comment on Y", "your planner changed" — so `realtime.rs`
collects them for 1.5 s and the workspace refetches what's on screen: open
task tabs, the list in front, the inbox and timer (at most every 10 s). A
refetch that 404s is how revoked access shows up. Two sockets, because the
server keeps one watch per socket: one watches `org:<id>` (every task change
in the organisation), the other `task:<id>` for the front tab (comments only
go to stakeholders and whoever has the task open). After a drop, everything
on screen is refetched, since events were missed. The socket thread
(`api/socket.rs`) is blocking tungstenite with a short read timeout, pinging
every 30 s so a laptop waking up finds its dead socket.

The socket takes a session cookie or a personal access token, and a
header-mode client has no cookie, so the app mints a **read-scope** token
named "Desktop app" after sign-in, keeps it in the keychain next to the
session, and revokes it on sign-out. If it's revoked from the web, the app
mints one more, once. Notes, sparks, reminders and the knowledge base have no
events on the server, so they still refresh on ⌘R, focus and module switch.

**Articles are edited through the server's edit session**: autosaved PATCHes
of the revision it hands out; a 409 (someone started a newer session) stops
the editor rather than overwrite them, text kept on screen.

### Decisions worth knowing

**GPUI, not a webview.** The ask was Zed's speed and feel, and GPUI *is* Zed's
renderer: GPU-drawn, no browser engine. The release binary is ~18 MB and sits
around 45 MB of memory with notes open. The cost is a younger ecosystem — GPUI
Kit supplies the inputs, menus, dialogs and resizable panels; the tab strip,
notes list and quick open are written here.

**The token lifecycle is hand-rolled, as on mobile.** `st-auth-mode: header`,
`St-*` headers read off *every* response (the 2FA challenge rotates the token
on a 204), pre-emptive refresh from `Front-Token`'s `ate`, concurrent refreshes
collapsed into one, both gates (`st-mfa-ok`, `st-ev`) matched by shape. See the
mobile app's `NOTES-API.md`; nothing here needed a backend change.

**Tokens live in the OS credential store** (Keychain, Credential Manager,
Secret Service) via `keyring`, keyed by profile. An unsigned debug binary
changes signature every rebuild and macOS asks for the Keychain again each
time, so `AYEAYE_TOKEN_STORE=file` keeps them in a 0600 file for development.

**Notes are cache-and-refetch, never a local database.** The last list is
cached per profile and organisation so the window draws before the network
answers; every write goes to the server. That's the mobile app's rule and the
API's: access can change between reads.

**Autosave never drops a keystroke.** Edits are debounced (700 ms) into a PATCH
of only the changed fields; a failed save keeps them pending and retries every
5 s; closing a tab flushes; quitting waits for the last PATCH. A new note's tab
opens before its POST returns, and anything typed meanwhile is sent once it has
an id.

**Changes from elsewhere are adopted only when nothing local is pending.** A
refresh (⌘R, or returning to the window after 30 s) replaces an open note's
text if it changed on the server and you have no unsaved edits — cursor kept.
With unsaved edits yours win, as in the web app (the API has no version to
compare against). A note deleted elsewhere closes if clean, and offers "Keep as
a new note" if not.

**Blank titles aren't sent.** The server refuses an empty title, so clearing it
leaves the old one on the server and the tab says "Untitled note" until you
type one.

---

## Developing the UI without looking at it

`--features devtools` adds a scripted driver that renders frames straight to
PNG — no screen-recording permission, and the window stays hidden so it never
takes the keyboard from whoever is at the machine:

```sh
AYEAYE_CONFIG_DIR=/tmp/aa AYEAYE_TOKEN_STORE=file AYEAYE_THEME=dark \
AYEAYE_SHOTS=/tmp/shots \
AYEAYE_SCRIPT='wait 2000; keys cmd-n; type Hello; wait 900; shot hello; quit' \
  cargo run --features devtools
```

Steps: `wait`, `shot`, `keys`, `type`, `click`/`dclick`/`rclick` (window
points), `scroll`, `quit`. `AYEAYE_CONFIG_DIR` keeps a run away from your real profiles.

---

## Not done

- Live updates cover tasks, comments and the planner — what the server
  announces. Notes, sparks, reminders, the knowledge base and notifications
  have no events, so they refresh on ⌘R, window focus, module switch, and
  (inbox, timer) a minute's poll or a nearby task event.
- Not in tasks yet: attachments and files, dependencies, recurrence, tags,
  time entries, the board and calendar views, creating projects. Comments
  can't be edited or deleted here.
- The KB can't share books, manage access or attach files — the web app does.
- The inbox has been tested against an empty inbox only; opening a
  notification relies on its `link_path` (`/orgs/<org>/tasks/<id>`), anything
  else opens in the browser.
- No offline writes — a failed save retries, but quitting while offline loses
  what wasn't sent (the quit hook logs it).
- No 2FA enrolment, registration or password reset — those stay on the web.
  Signing in to an account with 2FA works.
- Not notarised; `make app` is ad-hoc signed. No app icon yet.
- Not yet built on Linux or Windows.
