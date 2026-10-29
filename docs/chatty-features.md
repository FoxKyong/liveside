# Chatty feature map (reference)

What [Chatty](https://github.com/chatty/chatty) does, as a checklist for
deciding what we build. Compiled 2026-09-22 from Chatty master `48f0c9e`
(v0.29-b1): its built-in help and source layout. Chatty is GPLv3: we take
**features and behaviour**, never code.

Scale for orientation: ~169k lines of Java in 638 files, 31 settings pages.

Legend for the "Plan" column: **done**, **MVP** = first usable version,
**later**, **?** = undecided, **no** = not planned. The plan notes were
last brought up to date on 2026-10-13.

## Twitch connectivity

| Feature | Chatty | Plan |
|---|---|---|
| Chat receive | IRC (`irc.chat.twitch.tv`, TLS) + IRCv3 tags | **done**, via EventSub instead |
| Chat send | IRC; replies via Helix | **done**, Helix |
| Login | OAuth implicit flow, local redirect server, user-chosen scopes | **done**, Device Code Flow |
| Mod actions, AutoMod, suspicious users, warnings | EventSub | **done** (`docs/roadmap.md`, sections 1 and 2) |
| Polls, raids, shoutouts, Shield Mode, channel point redemptions | EventSub | **done**, both the events and starting them (`docs/roadmap.md`, sections 3 and 4) |
| Stream status (title, game, viewers, uptime) | Helix, polled ~2 min | **done**: live or not in the channel list, viewers and how long it has run beside the account (both from Get Streams, once a minute), title and category changes said in the channel |
| Recent messages on join | recent-messages.robotty.de | **done differently**: local chat log only, nothing fetched (privacy) |
| Third-party emotes | FFZ (+ its websocket), BTTV, 7TV | **done** (codes recognized; images in the GUI, animated ones included; live updates later) |
| Emoji | Twemoji images, `:shortcode:` input | `:shortcode:` input **done** in both (Tab, GitHub's codes from the `emojis` crate); pictures come from the system's emoji font, so Twemoji images are not planned for now — they would mean another CDN and CC-BY attribution |
| Pronouns | api.pronouns.alejo.io | ? |
| Shared Chat (multi-channel streams) | source channel logo/badges | **done**: the source channel is marked on the message |
| Anonymous reading | IRC without login | partly: the IRC transport exists, used anonymously for the user list |

## Window, tabs, layout

- Tabs for channels and whispers; drag to reorder, move between panes,
  split side by side, tear off into popout windows.
- Dialogs (Channel Info, Admin, AutoMod, ...) can be docked as tabs.
- Saveable and restorable layouts (`/layouts add|save|load|remove`).
- Title bar: chatters | viewers, title, game, room modes (sub-only, slow,
  r9k, emote-only, shield, language). Room modes are **done** (header),
  and so are viewers and uptime.
- User list with status symbols (`~` broadcaster, `@` mod, `!` VIP,
  `%` sub, `^` bot, ...). Ours is **done** in both frontends (F3 in the
  terminal, the "People" button in the window): who has talked by default,
  or real membership over IRC when `user_list = "irc"`.
- Toggleable menubar, input bar, user list; "fullscreen".

Plan: tabs **done** in the TUI, channel list **done** in the GUI, whisper
threads **done** in both; popouts **done** in the GUI (a right click on a
channel in the list, or `/popout`: the channel's chat moves into a
window of its own with a line to type into, and back when it closes);
splits **done** the same way (**Show it beside the chat** in that menu
docks the channel's chat to the right of the one on screen, with its own
line and a × that puts it back; dragging the divider sets its width,
and the panes come back at the next start as they were left, as do
windows of their own, at their size). The same menu clears a channel's
chat on the reader's screen only, as `clear` does in a terminal; `/clear`
stays Twitch's, which empties it for every viewer. The TUI gets WeeChat-style buffers
and windows instead.

## Chat display and input

- Custom text pane: text + (animated) emotes + badges, reply threads,
  deleted/timed-out messages shown in place, configurable fonts and colours.
  Ours: animated emotes are **done** in the GUI, with a setting to stop
  them; fonts and per-user colours later.
- Readability correction of user colours; custom per-user colours.
- Pause scrolling while the mouse moves over chat; scroll up to stop
  autoscroll.
- TAB completion: nicks (predictive order), Shift-TAB emotes, `/` commands,
  `:` emoji, `.` custom completion.
- Input history (Up/Down, save unsent line with Down).
- Search in chat (Ctrl-F).
- User Info dialog: user's recent messages, ban/timeout buttons, colours.

Plan: messages with badges/emotes **done** in both, including readability
correction of user colours; replies **done** in both; completion of
names, emotes and commands and input history **done** in both (one
implementation in `liveside-client`); the user card **done** (a dialog in the GUI, `/user`
printed in the TUI); search in chat **done** in both (Ctrl-F: newest
match first, every match marked; the terminal selects the matched
message, so Ctrl-R answers it).

**Links in chat — designed, not built.** The design (finding links,
opening them only on confirmation by default, where a shortened link
leads, VirusTotal, a Security section in the settings) is in
`docs/architecture.md`, section *Links*.

## Moderation

- Built-in Twitch commands (`/ban`, `/timeout`, `/warn`, `/pin`, ...).
- One-click moderation (Ctrl+click), keyboard user selection (Ctrl-Space,
  then W/S/A/D, Q, E).
- AutoMod queue dialog with keyboard approve/deny.
- Moderation log, blocked terms, repeated message detection.

Plan: **done** in both frontends (the GUI through menus): `/timeout`,
`/ban`, `/unban`, `/warn`, delete message, `/clear`, `/announce`, the room
modes (`/slow`, `/followers`, `/subscribers`, `/emoteonly`, `/uniquechat`
and their `off` forms) plus the moderator-only `/delay`, `/mod`, `/unmod`,
`/vip`, `/unvip`, `/pin`, `/unpin`, `/title`, `/game`, `/shoutout`,
`/raid`, `/unraid`, `/marker`, `/commercial`, `/shield`, the blocked-term
commands (`/blockterm`, `/unblockterm`, `/blockedterms`) and the suspicious
-user ones (`/monitor`, `/restrict` and their lifts), with keyboard message
selection and Ctrl-T/B/D and Alt-P preparing a command.
The AutoMod queue, unban requests and the moderation log are **done** too,
in the chat and in the moderating panel. Repeated message detection is
**done** in the engine: a message somebody says again, alike enough, is
marked "(×3)" in both frontends and matched by `config:repeatedmsg` and
`config:repeatedmsg|N`, with the threshold, similarity, time window and a
restricting rule in the settings. Chatty's choice of comparison methods
and ignored characters are not; whitespace and case are always ignored. `docs/architecture.md` has the full comparison against Twitch's
API, and `docs/roadmap.md` what is left of it.

## Customization (Chatty's strongest area)

- **Highlight / Ignore / Filter** rules with a small prefix language:
  matching (`user:`, `reuser:`, `status:`, `cat:`, `chan:`, `start:`,
  `re:`, ...) and behaviour (`color:`, `bgcolor:`, `config:silent`,
  `config:hide`, `to:<custom tab>`, `ncat:` notification category, `n:`
  note). Highlighted and ignored messages collected in their own panes.
- **Custom Tabs / routing**: copy messages matching rules into named tabs.
- **Custom commands** with replacements (`$(...)`, functions, `$request()`),
  chaining, foreach, delayed execution; custom context menus and user
  dialog buttons.
  Ours: the commands themselves are **done** in Chatty's syntax
  (`[commands] custom`, a list in the settings, checked as written): the
  numbered words with `-` and the required `$$`, `$(chan)`, `$(chans)`,
  `$(my-nick)`, the `stream…` values, `$if`, `$ifeq`, `$lower`, `$upper`,
  `$trim`, `$join`, `$rand`, `$randnum`, `\$` and `$"…"`, a command for
  one channel (`/hi#harborlight …`), and `/echo`. Not done: Chatty's other
  functions (`$request`, `$datetime`, …), `/chain`, `/foreach`, `/runin`,
  context menus and buttons, and `//` anonymous commands, since `//` here
  sends a line starting with `/`. `/me` is **done** both ways: shown as
  an action, and sent over IRC since Twitch's API for sending chat does
  not take it (`docs/architecture.md`, *Twitch transport*).
- **Addressbook**: user categories, usable in rules and `/join [category]`.
  Ours is **done**: `[addressbook]` in the settings, a list to edit in
  the settings screen, Chatty's `/ab` commands, `cat:`/`chanCat:` in
  rules and `/join [category]`. Not done: `[*]` favourites, `live` in a
  join, mod commands, the import file, colours and badges by category.
- Custom names, local emotes (`emotes.txt`), ignored emotes, message colours.
- Hotkeys, including global ones.
  Ours: hotkeys are **done** in both frontends, as `KEY what to do`
  lines (`[hotkeys] bindings`, edited in the settings): the line runs as
  if typed, custom commands and their replacements included, with `$1`
  the user of the selected message (Ctrl-Up/Down in the terminal, the
  message last clicked in the window). F1–F12 with any modifiers, and
  letters or digits with Ctrl or Alt; F2, Ctrl-C and Ctrl-Q stay
  theirs. Global hotkeys (when the window is not focused) are not done:
  Wayland gives them to the desktop, not to applications.
- `/set`, `/add`, `/remove` for every setting. (Ours: a settings screen on
  F2 writing `config.toml`; commands for settings are not planned.)

The rule language is **done** for highlighting, ignoring and filtering,
in Chatty's own syntax so a list kept there carries over: the text
prefixes (`text:`, `cs:`, `w:`, `wcs:`, `start:`, `startw:`, the `reg`
family, `msg` variants, `!` and `+`), `user:`, `reuser:`, `chan:`,
`status:`, `blacklist:`, `config:` (`info`, `any`, `firstmsg`, `hl`,
`url`, `historic`, `b|`, `restricted`, `shared`, `live`, `silent`,
`!notify`, `!ignore`), `color:`, `bgcolor:`, `replacement:`, `n:`, and
the address book's `cat:`, `chanCat:` and `chanCat2:` with their `!`
forms. The engine applies them, so every frontend gets the same; ignored
messages are hidden and counted in the channel every 30 s. Not done: a
user's past messages (`msgs:`), `if:`, message tags, custom tabs (`to:`),
`ncat:`, presets, follow-up highlighting, and ignoring whispers. What lit up and what was hidden is
**done** as Chatty's two lists: a window with a page for each, buffers
in the terminal (`/highlights`, `/ignored`); ignoring somebody is an
item in a message's menu and `/ignore`, `/unignore`. Chatty's separate
"ignored users" list is this same ignore list. `docs/architecture.md`,
*Rules*, has the details.

## Streams and notifications

- Live Channels list (joined + followed), notifications on status changes.
- Notifications and sounds for highlights and events; TTS.
- Channel Info dialog with viewer history graph.
- Followers / subscribers lists.
- Admin dialog: change title, category, tags, presets; run and schedule ads.
- Stream highlights file and stream markers.
- Streamlink integration, `/proc` to run processes.
- Stream Chat overlay window.
  Ours is **done** in the window: a right click on a channel in the list,
  or `/streamchat`, opens a window of that channel's messages alone, in
  one colour (`[stream_chat] background`) for a streaming program to key
  out, without times, each for `seconds` (0 keeps them), at `font_size`.
  What a moderator deletes, or a timeout or ban removes, is taken off at
  once. Not done: several channels in one, channel logos, the
  `/setStreamChatSize` family.

Streamlink is **done**: ▶ beside the account in the window, `/stream
[quality]` in the terminal, with the command, quality, player and any
further arguments in a Streamlink section of the settings. It runs on
the machine the frontend runs on, not in the engine, and a Streamlink
that ends with an error, or so soon that the player cannot have played
(which Streamlink does not count as an error), says why in the channel.

Notifications and sounds are **done** for five things: mentions and
highlights, whispers, a joined channel going live, a followed one going
live, and moderation (a
message held, an unban request). By default only for what is not on
screen already; the window raises a desktop notification and plays a
sound from the system's sound theme or a file of one's own, the terminal
rings its bell and raises the same notification. Clicking one opens the
channel or whisper it is about. `docs/architecture.md`,
*Notifications and sounds*, says why it is done the way it is. A rule
can keep its highlight quiet (`config:silent`, `config:!notify`); TTS and
notification categories (Chatty's `ncat:`) are not done.

Plan: the admin side is **done** through Twitch's API (`docs/roadmap.md`,
section 4): the channel form for title, category, tags and the rest,
`/ads`, `/snooze` and `/commercial`, the schedule, `/marker` and clips.
The followed-channels list is **done**: the live ones with category,
title, viewers and uptime, then the rest, each to join (a window from the
channel list, `/followed` in the terminal). The engine checks every
minute which followed channels stream; one that begins is said in the
status view and notified (its own switch, *Followed channels*), and an
open window keeps up. Chatty's list also mixes in the joined channels;
here those show their state in the channel list. The followers and
subscribers lists are **done**: `/followerlist` and `/sublist` (in the
window also **Moderate → Followers…** and **Subscribers…**), the newest
500 with how many there are in all, since when each follows, each
subscriber's tier and who gave it, and the subscriber points. Twitch
names followers only to the channel and its moderators and tells
everyone else the number, which is what is shown then; subscribers are
the channel's own. They are not `/followers` and `/subscribers`, which
stay the chat modes. The viewer graph is **done**: clicking the viewer
count over the chat (or `/viewers`) opens a graph of the stream so far,
minute by minute, with each change of title or category marked and
listed, and how many watched at the minute under the pointer; in the
terminal `/viewers` draws the same graph in braille dots over the chat,
with the changes as lines of another colour, until Esc.

## Persistence and system

- Settings directory, `-d` / `-portable` options, single-instance mode
  forwarding arguments to the running instance.
- Chat logs to files, highlighted/ignored logs.
- Channel favourites and join history.
- Launch options (`-connect`, `-channel`, `-set:...`, `-cc`).

Plan: chat logs **done** (JSON, and the source of history on join);
config file **done** (TOML, with a settings screen in both frontends);
launch options partly (`--config` in both, `--channel` and `--login` in
the TUI only);
single-instance is covered by the daemon design.
