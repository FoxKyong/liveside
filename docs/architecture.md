# Architecture

Status: the terminal frontend talks to real Twitch chat (login, join,
send, moderate, third-party emotes, room modes, history on join), through
the daemon or in one process. The graphical frontend shows chat, joins and
sends, and is where the work is now. Decisions below
were made on 2026-09-22 and are the baseline until explicitly revised
here.

## Goals

- Standalone Twitch chat client, functionally inspired by
  [Chatty](https://github.com/chatty/chatty), written from scratch in Rust.
- Two frontends from day one: **terminal (TUI)** in the spirit of WeeChat,
  irssi, aerc or profanity, and a **graphical (GUI)** one. The GUI may offer
  more than the TUI; both are views over the same engine.
- The engine can run as a **daemon**, with frontends attaching to it (like
  WeeChat's relay). Running everything in one process must work too.
- Linux is the primary platform. Other platforms only where the chosen stack
  makes them cheap.

## Layers

```
          ┌──────────┐   ┌──────────┐
          │  liveside-tui  │   │  liveside-gui  │        frontends: render + input only
          └────┬─────┘   └────┬─────┘
               └──────┬───────┘
                ┌─────▼──────┐
                │ liveside-client  │   Command → / ← Event, in-process or socket
                └──┬──────┬──┘
     in-process    │      │   Unix socket
                   │   ┌──▼────────┐
                   │   │ liveside-daemon │
                   │   └──┬────────┘
                ┌──▼──────▼──┐
                │  liveside-core   │   state, rules, commands, settings, logs
                └─────┬──────┘
                ┌─────▼──────┐
                │ liveside-twitch  │   OAuth, EventSub, Helix, BTTV/FFZ/7TV
                └────────────┘

   liveside-proto: Command, Event and the data model, used by every layer above core
```

| Crate | Kind | Responsibility |
|---|---|---|
| `liveside-proto` | lib | `Command`, `Event`, message model. Plain data, serde. No I/O. |
| `liveside-twitch` | lib | Talking to Twitch and emote providers. No knowledge of frontends. |
| `liveside-core` | lib | The engine. Single owner of state. |
| `liveside-client` | lib | What frontends use; same API for in-process and daemon mode. Also holds the daemon side of the socket framing, so both ends agree. |
| `liveside-daemon` | bin | Hosts `liveside-core`, serves frontends over a socket. |
| `liveside-tui` | bin | Terminal frontend. Must not pull in GUI dependencies. |
| `liveside-gui` | bin | Graphical frontend. |

The project is called Liveside, and every crate, binary and directory
carries that name. It says what the client is for — the chat beside a live
stream — without naming Twitch, whose trademark rules do not allow its name
in the name of an app.

## Engine ↔ frontend contract

This is the part that makes the daemon possible, so it is strict:

1. Frontends talk to the engine **only** via `Command` (request) and `Event`
   (notification). No shared memory, no callbacks, no engine types that are
   not in `liveside-proto`.
2. Both are serde-serializable plain data. In-process mode passes them over
   channels; daemon mode sends the same values over a socket.
3. Every event carries a monotonically increasing sequence number. A frontend
   that attaches (or reattaches) first gets a **snapshot** (open channels,
   channels still being joined, recent scrollback, account state) tagged
   with a sequence number, then the live events after it.
4. Every command carries a request id and gets exactly one `response` with
   that id (no `error` field means success), sent only to the frontend that
   asked. **The response comes after every event the command caused**, so
   after `join` succeeds the channel is already known. Implementation: the
   backend reports results on the same queue as its events, and the session
   flushes events up to the command's completion sequence number before
   sending the response.
5. Several frontends may be attached at once (TUI over SSH and GUI at home).
   Per-frontend state (scroll position, selected tab) stays in the frontend;
   shared state (joined channels, read markers if we add them) is in the engine.
6. The session starts with a version handshake so frontends and daemon can
   refuse mismatched versions cleanly.
7. A frontend too slow to keep up (its event buffer overflowed) is sent a
   fresh snapshot. Frontends must accept a snapshot at any time and replace
   their state with it.
8. Problems that belong to no channel travel as `Log` events, which
   frontends show in their status view. **Engine-side code must not print
   to stderr**: with the engine in the same process as a terminal
   frontend, stray output corrupts the screen. (Found exactly that way:
   failed emote requests were being drawn over the chat.) The daemon's own
   process may print, and the transport does so only on its side.

Transport (decided): **newline-delimited JSON** over a Unix socket at
`$XDG_RUNTIME_DIR/liveside/daemon.sock` (directory 0700, socket 0600). Readable
with `socat - UNIX-CONNECT:...`. A binary format can come later if it ever
matters. Open: whether remote access goes through SSH forwarding only or
gets its own authenticated transport.

**When the daemon goes.** The terminal client says why in the shell and
exits with 1. A window keeps what was read, greys every channel, refuses
typed lines (Up brings them back) and tries the socket again, after 1 s
and then doubling up to 8 s. A daemon that comes back starts with no
channels, so the window joins again those it still shows, and leaves
out the history the join brings back, as it is already on screen. A
channel the daemon has by then (another frontend joined it) comes in the
snapshot instead, which replaces its lines. A daemon that refuses the
window, after an upgrade to another protocol, is not asked again.

The engine talks to Twitch through a **backend** interface
(`liveside_core::backend`). A demo backend (`liveside_core::demo`) fakes Twitch with
generated chatter, so frontends and tests work without a connection to Twitch or
an account. Pictures are another matter: its emotes and badges are real ones,
whose images the window fetches from Twitch's CDN like any others.

## Twitch transport

Twitch's own docs: "The preferred method of viewing and sending chats on
Twitch is through EventSub and Twitch API" (dev.twitch.tv/docs/chat, checked
2026-09-22). Therefore:

- **Receive** chat and events via **EventSub WebSocket**
  (`channel.chat.message` and friends). Messages arrive as structured
  fragments (text, emote, mention, cheermote), no IRC tag parsing.
- **Send** via **Helix** (`POST /chat/messages`); moderation and everything
  else via Helix too.
- **IRC** only for what the newer interfaces cannot do: the user list
  (membership, optional, anonymous unless the login has `chat:read`), and
  sending `/me`, which Helix does not take (below). Chat is not read or
  sent over it otherwise. It may still come back as a transport for
  anonymous (logged-out) reading, behind the same interface.

Verified limits to design around:

- Max **300 enabled subscriptions per WebSocket connection**; one channel
  needs several subscription types, so the engine must spread subscriptions
  over connections and track them.
- A new connection must create a subscription within **10 s** or Twitch
  closes it.
- Reading chat via EventSub needs a logged-in user token.
- EventSub has no JOIN/PART, so there is no Chatty-style user list from
  presence; recent history on join comes from a third party
  (recent-messages.robotty.de), as in Chatty.

Authentication: **Device Code Flow** (no local HTTP server needed, works in a
terminal and over SSH), verified against Twitch on 2026-09-22.

Tokens are stored in `$XDG_CONFIG_HOME/liveside/token.json`, file mode 0600 in a
0700 directory. A token can chat as the account, so moving it into the
system keyring is still open. On start the stored token is validated, then
refreshed if that fails; if the refresh is refused (public clients have no
secret) the token is dropped and the status view says why, so the user
knows to log in again rather than finding the window simply logged out.
Without a network neither can be asked, and the token is kept for the
next start: it used to be dropped then too, and starting offline logged
the user out.

The Client ID Liveside logs in with is built in: Twitch calls Client IDs
public and lets them be embedded, and the Device Code Flow needs no
secret. Twitch also wants every application to have one of its own, and
may cut off one whose ID other applications use, so a fork or a build
passed on by someone else registers its own at dev.twitch.tv and sets
`twitch.client_id` in the settings. A stored token made with another ID
is told apart at validation (Twitch names the client it was granted to),
said to be from another Client ID, and the user logs in again, since
Twitch would refuse every call with it.

How the EventSub connection is run:

- One connection for all channels, five subscriptions per channel
  (message, delete, clear, clear_user_messages, notification), so the
  300-subscription limit allows about 60 channels. Spreading over several
  connections is still open.
- Any close, including Twitch's `session_reconnect`, is handled by opening
  a fresh connection and subscribing again, rather than by reusing the
  offered `reconnect_url`. Subscriptions die with the old session, so
  nothing is left behind, at the cost of a short gap.
- Joins asked for before the session is ready are held and completed when
  it is.
- Silence longer than the keepalive Twitch announces (plus 10 s) counts as
  a dead connection. Reconnects back off from 2 s to 16 s.

`/me`, both ways (checked 2026-10-17):

- **Received**, an action has no field of its own in `channel.chat.message`:
  its `text` keeps IRC's CTCP wrapper (`\u0001ACTION …\u0001`) and the
  fragments are without it, as shown on Twitch's developer forum when chat
  came to EventSub (2024-01-25). The mapping turns that into
  `MessageKind::Action`, and both frontends draw it as something the
  sender does: no colon, and the text in their colour.
- **Sent**, not through Helix: `POST /chat/messages` takes no commands,
  and drops `/me` or sends only what follows it (twitchdev/issues#1019,
  open since 2024-11). So it goes over IRC, as Twitch's IRC guide says:
  logged in as the account (`chat:read` to log in, `chat:edit` to send),
  with the `twitch.tv/commands` capability, `PRIVMSG #channel :/me …`.
  The connection is opened by the first `/me` and kept; it joins a
  channel the first time it speaks there, holds what is asked before it
  has logged in, and says Twitch's refusals (IRC `NOTICE`) in the
  channel. The line comes back through EventSub like any other the
  account sends. A login from before these two scopes is told to log in
  again (decided 2026-10-17). Tried on real Twitch on 2026-10-19: sent
  from the window, it came back as an action and Twitch's own chat showed
  it in italics.

## The graphical frontend

**GTK4** (`gtk4-rs`), chosen on 2026-09-25. The deciding reason is text:
Pango shapes Thai marks, Hangul blocks, Arabic and emoji the same way the
rest of the desktop does, and brings input methods and accessibility with
it. A chat client for the whole world cannot treat that as a detail. The
cost is the C libraries and a harder road to Windows, which Linux being the
primary platform makes acceptable.

Chat is a `GtkTextView` with tags rather than a list of widgets, for the
same reason: a whole line goes to Pango at once, so wrapping and shaping
are its business, not ours. Colours match the terminal, including lifting
a name colour too dark to read.

GTK owns the drawing thread and the engine keeps its Tokio runtime on
another, with two channels between them (`crates/gui/src/engine.rs`): a
widget is never touched from outside the main loop. The window otherwise
speaks the same protocol as the terminal frontend and reads the same
settings file, and both take their words from the same catalogue in
`liveside-client`, so the two interfaces cannot drift apart. The catalogue
is a TOML file per language (`crates/client/lang/`), built into the
program: a translator edits a list rather than Rust, a line a language
lacks is shown in English, and the tests compare every file with English.

Emotes are pictures. They are written as their code first and become a
picture when one arrives, so an emote whose picture cannot be had stays
readable instead of leaving a hole — which is what happens to a Twitch
emote whose id has been retired. Pictures are decoded **in Rust** (the
`image` crate) rather than by gdk-pixbuf, because 7TV serves WebP and AVIF
and the WebP loader is not installed everywhere; this way the same emotes
appear on any machine. They are fetched once, kept in
`$XDG_CACHE_HOME/liveside/emotes`, and drawn at the height of a line of text,
which Pango's metrics give.

Animated emotes (GIF, animated WebP, APNG) are decoded whole — every frame
composed onto the canvas, up to 240 of them — and played on the window's
own frame clock rather than on a timer of ours. The frame to show is worked
out from the clock's time, not counted up, which has three consequences
worth keeping: every copy of an emote is on the same frame, a slow moment
does not leave one behind, and a picture that GTK is not drawing costs
nothing, because the clock does not run for an unmapped widget. A picture
scrolled out of the visible area is skipped as well: a text view keeps the
widgets of lines far above, and giving them new frames sixty times a second
would be work for nobody. A file that claims to be animated but cannot be
read frame by frame falls back to a still picture rather than to nothing.

`[ui] animated_emotes` turns the movement off, and does so at once: the
pictures already on screen are taken off the frame clock and parked on
their first frame, because a face caught mid-blink is not what the emote
is. A moving chat is not to everyone's taste, and standing still costs a
laptop less — with the emotes of a full window stopped, this client goes
from a few per cent of a processor to none.

What fails is said in the chat, not in the status line: a notice with a
time, in colour, wrapped, scrolling away with everything else, as the
terminal frontend has always done. The status line keeps to the account and
the connection and is cut short rather than stretching the window. Text
Twitch worded itself has no code to translate, so it is wrapped in a
sentence that does — "Twitch refused it: …" — rather than appearing as a
bare English line in a Czech window.

Settings and moderation are there. The settings window is built from the
same rows as the terminal one (`liveside_client::settings`, moved there for this)
and draws each kind as the widget a graphical program uses: a switch, a
spin button, an entry, a drop-down. Neither frontend can offer a setting
the other lacks, and both write the same file. Each row's help is the
tooltip of the whole row, not only of its name: a setting that opens a
second connection to Twitch should say so wherever the pointer rests.

Moderation lives in two places. The **Moderate** button holds the room's
restrictions and clearing the chat; each item is a toggle, and the engine's
report of the new state is what the next click reads.

A right-click on a message opens a menu the client builds itself, for the
moment it is opened. The view's own menu is not used: in text that cannot
be edited, Cut, Paste, Delete, Undo and Redo can never do anything, and
they were only in the way. Every item appears only when it would act on
something:

- **Copy the selection**, when something is selected.
- **Copy this message**, whenever the pointer is over one. In a chat that
  is moving, catching a single line with the mouse is a losing game, so the
  whole line can be had without selecting it. Emote pictures come back as
  their codes, not as the placeholder character that stands for a widget.
- **Reply, Delete, Time out, Ban**, in a section of their own headed with
  the author's name — the menu acts on one message, and a pointer is a poor
  way of saying which. They appear when the pointer is over a message and
  the selection, if any, lies inside that same message; with several
  messages selected a single-message action would act on something other
  than what is selected, so it is not offered. Ban asks first, being the
  one action that is awkward to undo in front of an audience; the rest act
  at once, a menu item being a deliberate click already.

The menu is built from widgets rather than from a `GMenu`. A menu model's
items are driven by actions looked up by name, and an action the popover
cannot find leaves an item that silently does nothing — which is exactly
what happened here, and cost an afternoon. Widgets carry what they do.

Reading back works as in the terminal. The newest line is followed only
while the reader is at it; once they scroll up, the text stays where they
put it and a button at the bottom says how many messages have arrived since
(Esc, or the button, returns to the newest). Trimming waits too: dropping
old lines from the top would shift what they are reading, so a buffer over
its limit is cut only once they are back at the bottom. Sending a message
brings them back as well, since they will want to see it.

The people active in the channel are listed beside the chat, and the
"People" button puts the column away; a `GtkPaned` then gives its space
back to the text. Clicking a name writes `@name` into the input line,
which is how one answers somebody in a chat that moves. The list is
looked at once a second rather than after every message: redrawing it as
fast as chat arrives would be work for nothing, and would move the names
under the pointer.

## Stream Chat

A window for a stream to show, as Chatty's: one channel's messages,
nothing else, in one colour from edge to edge so a streaming program can
key the background out. It reuses the chat view, told to leave the times
off and draw links as text, with the palette that reads on the chosen
background, and a style sheet for the colour and the size of the text.

It keeps the lines it shows and draws them again when one goes: when it
is older than `[stream_chat] seconds`, or when a moderator deletes it or
times out or bans its sender. On the main chat such a line is struck
through for the moderator; on stream it is gone, since the audience is
who reads it. Only live messages go there: history, notices and what the
rules hide stay out. It starts empty, with what is said from then on.

## A channel in a window of its own, or beside the chat

A popout moves the channel's chat view out of the main window's stack
into a window of its own and back when that closes; docking moves it
into a pane beside the stack in the main window, and back when its × is
pressed. Both are a `Detached` channel
with the same column (the view and a line to type into). The rest of this
holds for both. The view is moved: moved, not copied,
so it keeps its lines, its search and its links, and goes on taking
lines from the same place. It is an application window rather than one
held above the main window, so it can go to another screen.

What is typed there is handled by the same code as the main window's
line, told where it was typed (`Place`): the channel it sends to, where
what the window says back goes, and whether the main window's reply in
progress goes with it (only from the main window). The main window,
showing that channel, says it is elsewhere and offers to show the window
or bring the channel back; menus and lists there still act on it. A
notification counts a popped-out channel as on screen while its window
is the one in front, and a docked one while the main window is
(`Seen::beside`). Hotkeys are the main window's.

The docked panes are a chain of dividers (`gtk::Paned`), each pane the
end of one whose start is everything on its left, so dragging a divider
moves only the pane to its right, and a wider window widens the chat
rather than the panes. A new pane gets as much as the chat and each pane
already there. The panes, their order and the widths they were dragged
to are kept across restarts in `$XDG_STATE_HOME/liveside/layout.json`
(`liveside_client::layout`): state rather than a setting, so not in
`config.toml`, which people edit and copy, and a file that cannot be read
is reported and ignored rather than stopping the window. The demo keeps
its own file. Channels are joined in no fixed order, so each one docked
last time is put in its place among the panes already back as it
arrives. A pane the window is too narrow for is squeezed, which is not
kept, and grows back to its width as room comes. Only a width the
reader drags to is kept: a press and a release on the divider with the
pane's width changed in between, watched on the events' way down,
since on Wayland the pointer's state does not say a button is held.
Closing a pane or leaving the channel takes it out of the file.

Channels in windows of their own are kept in the same file, with the
window's size; not its place, which a program cannot set on Wayland.
Closing the main window closes the program, so it destroys those windows
rather than closing them (closing one brings its channel back and takes
it out of the file) after noting their sizes, and they open again at the
next start. Closing one of them, or leaving its channel, takes it out.

A chat view that is made narrower or wider wraps its lines again, and
GTK moves the scroll while it lays them out; that read as the reader
scrolling up, so a view that was following the newest line stopped. A
change of width (the horizontal adjustment's page) now puts a following
view back at the bottom once the text has settled. Only the width: the
height changes all the time, as GTK lays out text in the background.

## Light and dark

`[ui] theme` is `system`, `light` or `dark`. The graphical client asks GTK
to paint itself the chosen way and repaints the chat with it; `system`
leaves GTK alone and then asks it what the desktop decided, rather than
guessing.

The terminal client takes its colours from the terminal, but corrects
the ones it chose itself for the background they are drawn on: a
viewer's colour is blended away from the background until it can be
read, and history fades towards it. A terminal does not say which
background it has unless asked, so with `system` the client asks it at
start (OSC 11, through `terminal-colorsaurus`, before the screen is taken
over and keys are read); a terminal that does not answer is taken to be
dark, as most are, and `light` or `dark` says it outright. On a light
background the palette's pale colours (grey, white, the light tints) are
drawn as their deeper siblings, but only on the terminal's own
background. Until 2026-10-28 only dark backgrounds were corrected for,
and on a light one names stayed too pale to read.

The chat is not GTK's to colour: the time, notices, badges, emote codes and
the colour a viewer chose for their name are ours. Each theme therefore
carries a palette (`crates/gui/src/theme.rs`), and the text carries tags
rather than colours, so changing the theme repaints the lines already on
screen. The one thing that does not change is the colour worked out for a
name when its line arrived — that stays as it was, as older lines keep
their wording when the language changes.

A viewer's colour is moved far enough from the background to be read
against it: up towards white on a dark background, down towards black on a
light one, keeping its hue either way. Before this, both frontends lifted
colours as if the background were always dark, which was true of the
terminal and not of the white window.

## Language

The interface can be shown in more than one language. Every sentence the
frontends own is a field of `liveside_client::lang::Strings`, read from
the language's file (`crates/client/lang/<code>.toml`); a line a file
lacks is shown in English, and the tests find every such line and every
placeholder a translation drops. `{name}` placeholders are filled at the
call site. The language comes from `[ui] language` in the settings file
(`auto`, or a language's code: `en`, `cs`, `es`, `zh-Hant`, …), where
`auto` reads `LC_ALL`, `LC_MESSAGES` and `LANG`. Chinese has two codes,
one per way of writing it, and the locale's region picks between them:
Taiwan, Hong Kong and Macau read the Traditional characters (`zh-Hant`),
everybody else the Simplified ones (`zh-Hans`). Changing it in the
settings screen applies at once, since the text is looked up while
drawing.

The terminal lays text out in columns, as the terminal draws it, not in
letters: a Korean or Chinese letter takes two columns, a Thai vowel mark
none. A wide letter just left of a box drawn over the chat is replaced by
a space, since its second half would lie on the box's border and the
terminal would never be told to draw it.

Each language is listed where it is chosen by its own name (*Čeština*,
*Español*), which is what somebody looking for theirs recognizes, so no
language has to name all the others. A machine translation nobody has
checked says so beside its name, in the language itself (*Español
(traducción automática)*), from `machine` and `note` in its file. English
and Czech are checked by the owner; the others are machine translations
until a volunteer has checked one (`roadmap.md`, *Languages*).

Arabic is written right to left (`rtl = true` in its file). The window
is then laid out from the right, by GTK's default direction, set at
start and whenever the language changes; each chat line still takes its
direction from its own text. Within a line, a name and a message are
each set apart in a Unicode first-strong isolate, so the direction one
runs in cannot turn the rest round (an English question in a line that
starts in Arabic keeps its question mark at its end), and a direction
mark hidden in a message stops where the message does. Copying leaves
the isolates out. The terminal gets the text as it is, in
logical order: a terminal that does bidirectional text itself (Konsole,
seen 2026-10-23; GNOME Terminal's VTE is said to) shows it right, one
that does not (Alacritty, seen) shows it backwards and unjoined.
Reordering it in Liveside instead would turn it backwards again in the
terminals that do it right.

The header's buttons cut their words short with an ellipsis, the full
words in their tooltip, rather than widen the window: Spanish's longer
words ran the last button past its right edge at the default width.

Text the engine writes is translated by the frontend, not by the engine:
with a daemon serving several frontends, the language belongs to whoever
shows it. `CommandError`, `LineContent::Notice` and `Event::Log` therefore
carry a **code** (`channel.unknown`) and **named arguments**
(`channel=orbitcast`) beside the English sentence. A frontend says it in its
own language when it knows the code, and shows the English otherwise, so an
old frontend and a new engine still understand each other.

Twitch's own wording has no code and travels untouched
(`CommandError::passthrough`): it is not ours to translate, and inventing a
Czech version of a Twitch error would hide what Twitch actually said. A
test keeps every language knowing the same codes with the same
placeholders, since a code translated in one language only would show
English in the middle of a translated sentence.

Chat content is never touched: channel names, nicknames and messages are
data.

## Terminals

The terminal frontend is checked by hand in Konsole, Alacritty, kitty,
Ghostty and WezTerm (2026-09-24, all on Wayland): colours, badges, the
faded history, the settings box and the user list column render the same in
all five. Two things are worth keeping in mind when changing the drawing
code:

- Text must be measured in columns, not characters: emoji and CJK take two,
  and being one column out moves the user-list rule. A test renders lines
  with emoji, Japanese and diacritics at several widths and checks that the
  rule stays in its column.
- Anything drawn in a box has to be clipped to it. A long setting value ran
  past the border of the settings box — seen first in Konsole, because its
  window was narrower.

The terminal's own "dim" attribute is not used at all; see the history
section for why.

Checked with live chat in Korean, Japanese, Chinese, Russian, Thai and
Greek (2026-09-24): all render, including Thai vowel and tone marks above
and below the line and Hangul syllable blocks. That test found the tab bar
overflowing with a dozen channels, cutting the active tab in half; the bar
now scrolls to keep the active tab whole, with `‹` and `›` where tabs are
hidden.

## Moderation

Commands are parsed in `liveside_core::input` and become `BackendRequest`s, so a
frontend needs to know nothing about Twitch: it sends the typed line. Room
modes go through Update Chat Settings with **only the field being changed**
in the body, so two moderators changing different settings do not undo each
other.

### What Twitch has, and what is here

There is no single Twitch page called "all moderator commands". Two lists
together are the whole of it: the **Helix API reference**
(dev.twitch.tv/docs/api/reference — the Chat, Channels, Moderation, Raids
and Streams sections), and the help article **Chat Commands**, which is
what the web chat's `/` commands do. Read from the API reference on
2026-09-25, this is where this client stands.

Done: `/ban`, `/unban`, `/timeout`, `/untimeout`, `/delete`, `/warn`,
`/clear`, `/announce`, `/slow`, `/followers`, `/subscribers`,
`/emoteonly`, `/uniquechat` (and their `off` forms), `/delay`, `/mod`,
`/unmod`, `/vip`, `/unvip`, `/pin`, `/unpin`, `/title`, `/game`,
`/shoutout`, `/raid`, `/unraid`, `/marker`, `/commercial`, `/shield`,
`/blockterm`, `/unblockterm`, `/blockedterms`, `/monitor`, `/restrict`
and their lifts.

What is *not* here — the AutoMod queue, unban requests, whispers, the
broadcaster's own tools (polls, predictions, rewards, schedule) and the
lists (banned users, moderators, VIPs) — is written down with what each
one needs in `docs/roadmap.md`, together with everything else Twitch
offers that this client could usefully do.

Who may do what is not uniform, and Twitch's refusals are unhelpful about
it. Checked against the API reference on 2026-09-29:

- `/title`, `/game`, `/raid` and `/commercial` are taken **only from the
  channel's own login**: Modify Channel Information and Start a Raid both
  say the `broadcaster_id` must match the user in the token, and Start
  Commercial says outright that editors and moderators may not run one.
  The engine checks that itself and says so, because Twitch's "the ID in
  broadcaster_id must match the user ID in the token" is no use to
  someone standing in a friend's channel.
- That a moderator *does* change the title every day is not a
  contradiction: the change goes through a bot the broadcaster
  authorised, which is the broadcaster's own login as far as Twitch is
  concerned, or from the dashboard as an **editor** — a role of its own,
  separate from moderator, which Twitch's help article "Managing Roles
  for your Channel" lists as able to edit stream info. Neither is
  something a third-party client holding a moderator's token can do, so
  the message says where the change really comes from.
- `/marker` is the exception: Create Stream Marker takes the broadcaster
  **or one of their editors**, and nothing tells this client which
  channels it is an editor of, so it does not check and lets Twitch's own
  refusal — which names editors — through.
- `/commercial` additionally needs partner or affiliate status, which
  this client cannot know either; that one is Twitch's to refuse.

Each new command needs a permission the client did not ask for before, so
a token granted by an earlier version is missing them. Two things say so:
a refusal from Twitch that mentions scopes gets "log out and log in again"
added to it, and logging in with an older token now compares what was
granted against what is wanted and says up front what is missing — before
anything has failed.

In the terminal frontend, Ctrl-T, Ctrl-B, Ctrl-D and Alt-P write
`/timeout`, `/ban`, `/delete` or `/pin` for the selected message into the
input line instead of running them. A single keystroke that bans someone is
a keystroke that bans someone by accident, and the prepared line can still
be edited. (Pinning is on Alt because Ctrl-P already walks back through the
buffers.) The graphical client offers the same four in a message's menu,
and the room's own actions — the restrictions, clearing the chat, unpinning
and Shield Mode — in the Moderate menu. Shield Mode is two items rather than
a toggle: nothing tells this client whether it is on, and a toggle that
guessed would say "off" over a channel that is under attack.

## What a channel reports besides chat

Chat is what a channel must deliver; everything else it can say — someone
followed, the stream went live, the title changed, Shield Mode came on, a
shoutout went out, an advertisement started — is worth having and is not
worth failing a join for. Twitch refuses most of these unless the account
moderates the channel or owns it, which is the ordinary case for a channel
one merely reads, so they are subscribed after the required ones and a
refusal is passed over in silence (`EXTRA_SUBSCRIPTIONS` in
`liveside_twitch::eventsub`). The alternative — rolling the join back, as a
failed chat subscription does — would mean a viewer could not read a
channel because they are not its moderator.

Each event becomes a notice with a code and its values, the same way
errors are reported, so both frontends word it in the reader's own
language instead of repeating Twitch's English. The engine still sends the
English, for a frontend that does not know the code.

`channel.raid` is deliberately not subscribed: Twitch says it fires only
for raids that appear in chat, and those already arrive as chat
notifications, so subscribing would show every raid twice. Cheers are left
out for the same reason — a cheer is a chat message with bits on it.

Subscriptions are per channel and come and go with it, so the cost of all
this is one more list of ids to unsubscribe when leaving.

Whether a channel is streaming is state, not an event, so it is asked for
once when joining (**Get Streams**, which answers with a stream only while
one is running) and kept up to date by `stream.online` and
`stream.offline`. It rides in the snapshot as `ChannelState::live`, so a
frontend that attaches later knows it too. Both frontends put it beside
the channel's name: a filled dot when it is live, a hollow one when it is
not — told apart by shape as well as colour, since a colour alone is no
use to a reader who cannot see it or to a terminal that draws one style
per tab. A join that cannot ask is simply "not known to be live"; the
next `stream.online` corrects it.

## The AutoMod queue

AutoMod, and the channel's own blocked terms, catch a message before it
reaches the chat and wait for a moderator. Twitch reports those over
`automod.message.hold` and `automod.message.update` (version 2, scope
`moderator:manage:automod`), which are subscribed with the other extras
and refused for a channel one does not moderate — the ordinary case, and
harmless.

The queue is held by the **engine**, in each channel's state, so it
survives a frontend reconnecting and rides in the snapshot. A decision is
`Command::ResolveHeld`, and the engine takes the message off the list only
when Twitch says it is decided (`HeldResolved`), which is also how a
decision by *another* moderator reaches us.

The window writes the held message into the chat with **Allow** and
**Deny** as buttons in the line itself, through the same child anchors
emotes use: a queue of one is not worth a window, and the text to judge
is right beside the two answers. Both frontends also take `/allow` and
`/deny`, with a name or without — without one they answer the oldest,
which is how a queue is usually worked through. Those two are handled in
the frontends rather than in `liveside_core::input`, because answering them
needs the queue rather than the line that was typed.

## The moderation log

`channel.moderate` (version 2) reports every moderator action in a
channel — bans, timeouts, deletions, warnings, room modes, moderators and
VIPs given and taken, raids — in one subscription. It is the only way to
see what the *other* moderators are doing, and it is how a decision made
elsewhere reaches this client.

It is the hungriest subscription here: Twitch wants the whole
`moderator:read` family for it (blocked terms, chat settings, unban
requests, banned users, chat messages, moderators, VIPs), and refuses the
subscription outright if one is missing. The commands already ask for
most of them; `moderator:read:moderators` and `moderator:read:vips` are
asked for only because of this.

Each action becomes a notice with a code, as everything else does, so
both frontends word it: who banned whom and why, who deleted what, who
turned slow mode on and for how long. The restrictions are worded as
sentences rather than by Twitch's token for them — `slow` and
`emoteonlyoff` are what the API says, not what a person reads.

Actions this version has never heard of — and Twitch adds them — fall
back to `mod.other`, which says who did what by passing Twitch's own name
for the action through. Silence would be worse: a moderation log that
quietly drops what it does not recognize is not a log.

## Unban requests

The other queue, and the same shape: someone banned writes their case,
and a moderator answers. Twitch reports them over
`channel.unban_request.create` and `.resolve`, and the ones already
waiting are fetched when joining (**Get Unban Requests**, status
`pending`); **Resolve Unban Requests** answers one. Scope
`moderator:manage:unban_requests`, subscribed with the other extras and
refused where the account does not moderate.

As with held messages the engine holds the queue, the window puts the
request in the chat with the two answers beside it, and both frontends
take `/approve` and `/refuse` — a different pair of words from `/allow`
and `/deny` on purpose, because letting a message through and letting a
person back in are not the same decision and a queue of each can be
waiting at once.

## Our own messages held

`channel.chat.user_message_hold` and `.user_message_update` are about
**our** messages rather than other people's: they say that something we
wrote is being checked, and then whether it was let through. They need
only `user:read:chat`, so they work in any channel. Without them a
message simply never appears, which looks like the client losing it.

## Saying it in the reader's language

A sentence the engine composes is a sentence that can only ever be read
in English. Where one is meant for a person — a notice, a list of people,
what `/privacy` answers — the engine sends a code and the parts to fill
in, keeps its English text beside it, and the frontend says it properly,
falling back to that English when it does not know the code.

The parties in `/privacy` carry codes too, for the two that are not
names: "this machine" and "nobody" are not called that everywhere, while
7TV and Twitch are.

## What the channel says about itself

`/title` and `/game` set two fields of one Twitch call; the rest of that
call — the language, the tags, the stream delay and the content labels —
is a form, because those are set together and rarely: once, before going
live. **Moderate → Channel…** reads what is set now and writes back only
what was edited, since Twitch leaves untouched fields alone and a form
that always sent everything would overwrite whatever somebody changed
meanwhile. The terminal says it in a line with `/channel`.

The labels are a set of switches rather than a list: Twitch wants every
one of them on every change, and clearing them means saying "off" to each.

## Where a typed command is understood

A command that is only a request belongs in the engine's parser, where
both frontends get it from one place. The terminal keeps the few that
need what it alone knows: `/poll` and `/pred` with no arguments say how
the one on screen stands, and `/queue`, `/fulfil`, `/refund` and
`/payout` take the number of a row as that terminal has printed it.

Anything else typed in a channel is passed to the engine, so a command
added there works in the window and in the terminal at once — which is
how `/clip`, `/channel`, `/rewards`, `/newreward`, `/user`, `/poll`,
`/prediction`, `/endpoll` and `/lock` are parsed.

## Advertisements, and what the moderation log already covers

`/ads` says when the next break is due, how long it runs and how many
times it can still be pushed back; `/snooze` pushes it back by five
minutes and answers the same way, because what anybody wants to know
afterwards is when it is due now. Twitch writes "no break scheduled" as a
zero time rather than as nothing, so that is read as none.

Shield Mode is asked for on joining as well as subscribed to, and the
answer is kept: the two events say when it changes, and a client started
in the middle of an attack would otherwise show the channel as calm. The
menu offers one item that says which way it would go rather than two
that guess — the thing the roadmap wanted all along. It is rebuilt when
the state or the channel changes rather than as it opens, because setting
a menu model on a popover that is opening closes it again.

A Shared Chat session and a Hype Train are asked for on joining too, for
the same reason: an event only says when something changes.

Two things the roadmap listed are deliberately not subscribed to.
`channel.moderate` already reports every moderator action in the channel,
including bans, timeouts, and moderators and VIPs being made and unmade —
so `channel.ban`, `channel.unban`, `channel.moderator.*` and
`channel.vip.*` would say everything a second time.

## The channel list, while it is still filling

A channel takes a while to join: its name is looked up, half a dozen
subscriptions are made, the chat settings read, and what is already
waiting in it asked for — and the channels are done one after another.
With four of them in the configuration the window stood empty for most
of a minute, which reads as an application that has hung.

The window therefore draws a row for every channel it has asked for, at
once and faded, and that same row becomes the channel when it arrives, so
the list does not jump about. A join that is refused takes its row with
it: the engine names the channel in the refusal, which is what the window
reads to know which row to drop. The terminal frontend still shows a tab
only once the channel is there.

Whether a channel is streaming is asked again after every reconnection,
not only on joining. `stream.offline` says a stream ended, and a client
that was not listening at that moment — a token that expired overnight,
a dropped socket — would otherwise keep a green dot beside the name for
days.

## Whispers

A whisper is a channel without the moderation, so it gets what a channel
gets: a buffer of its own, one per person, with what was said in both
directions. The engine keeps the threads and sends them in every
snapshot, bounded by the same scrollback as a channel; the window lists
them under the channels as `@login`, and the terminal as another tab.
Typing in one whispers that person; `/w <name> <message>` works from
anywhere, including before any thread exists.

Whispering belongs to the account rather than to a channel, which is why
both frontends answer `/w` themselves instead of handing it to the
engine's channel parser, and why `user.whisper.message` is subscribed
once per session rather than once per channel.

Twitch will not carry a whisper from an account without a verified phone
number, and says so only when it refuses one. That refusal is passed on
as it comes: inventing a friendlier sentence for it would hide which of
the several reasons it was.

## Shared Chat

When two channels share a chat, the other channel's messages arrive here
as if they were this one's. The protocol has carried where a message came
from for a while; both frontends now say it, as a dim `#channel` before
the name. Without it a shared chat is a crowd rather than a conversation:
somebody answers a question nobody here asked.

The session beginning and ending is said too, with who else is in it —
this channel is left out of that list, since the reader knows where they
are.

## Who Twitch does not trust

A suspicious user's message carries why: likely a ban evader, banned in
channels that share bans with this one, or flagged by a moderator here —
and whether a moderator is watching them or has restricted them. It goes
on the message, in front of the name, because the reason is only worth
anything beside what they actually said. A list somewhere else would be
read long after this had scrolled past.

Twitch sends the message and the suspicion as two events in no fixed
order. A suspicion that arrives first waits for its message — in the
engine, which attaches it to the line before anybody sees it — and one
that arrives second marks the line already shown. Both frontends do the
second half themselves, since their own buffers hold the line by then.

The reason is put into words in one place, shared by both frontends, so
"nejspíš obchází ban" is written once rather than twice.

## The money

Subscriptions, gifts, bits, Hype Trains, goals and charity donations are
all the same small job: subscribe, and write a line into the channel. The
wording lives with every other sentence, so they arrive in the reader's
own language like everything else.

Three decisions worth writing down. A gift subscription arrives twice —
once for the gift and once for each person who got one — so the per-person
event is dropped: a wave of a hundred gifts would otherwise be a hundred
lines naming strangers. Somebody who gave anonymously is called that,
because Twitch does not say who they were and inventing a name would be
worse than the word. And money arrives as whole units with the decimal
point written down beside it, so 12.34 EUR comes as 1234 with two places
and is put back together before it is shown.

Hype Train progress is not subscribed to, only its beginning and its end:
the line that matters is "this is happening" and "this is how far it got",
and everything between is a bar nobody is reading in a chat window.

## The schedule

`/schedule` says when the stream is next, and **Moderate → Schedule…**
opens the list that changes it. Twitch separates calling off one
occurrence of a weekly broadcast from removing it altogether, and so does
the window: the first is "something came up this week", the second is
"this is not a thing any more". Being away is the third button, because
a week off is said once rather than by cancelling seven broadcasts.

A time is typed as `2026-10-07 18:00` and refused when it is not: a
half-typed time must never quietly become a broadcast at midnight. The
time zone sent with it is the machine's own, read from `/etc/localtime`,
falling back to UTC — Twitch wants an IANA name, and guessing one would
be worse than saying UTC and being visibly wrong.

## Clips

**F4** in either frontend clips the last ninety seconds or so of the
stream, as does `/clip` and **Moderate → Clip that**. The same key in
both, because clipping is the one thing that cannot wait for a menu to
be found.

Twitch answers before the clip exists — making one is asynchronous — so
the line says where the clip will be rather than that it is there. The
URL is the one for trimming and titling it, which is what anybody who
just clipped something wants next; it is good for a day, or until the
clip is published.

## Channel points

What people spend their channel points on is said in the channel, with
what the reward cost and whatever they typed into it — a reward that asks
a question is answered in the redemption, not in chat, so the answer
belongs on the same line.

Answering one — done, or the points given back — is a queue, and it sits
in the moderation panel beside the messages AutoMod is holding, because
it is the same job: a short list of things waiting for a decision, each
with two buttons. In the terminal it is `/queue`, with `/fulfil <number>`
and `/refund <number>`.

What is in that queue is limited by Twitch, not by this client: **only
the application that created a reward may list or answer its
redemptions.** A reward made in the dashboard can be seen, said in the
chat and listed, and nothing more — so only redemptions of rewards made
from here ever reach the queue, and `/rewards` marks the others as not
ours. That is why adding a reward is part of this feature rather than an
extra: **Moderate → Rewards…** lists what the channel offers, adds one,
removes one and changes one, and `/newreward <cost> <name> [| question]`
does the same from either frontend.

Pausing is a button of its own rather than a field in the edit form:
it is the one change a stream is actually interrupted for — stepping
away, and nobody should be able to redeem the thing meanwhile. The same
from the keyboard is `/pausereward <name>` and `/resumereward <name>`,
and `/rewardcost <points> <name>`, all by name, because a Twitch id is
not something anybody types into a chat.

The queue is kept by the engine, in the channel's state and in every
snapshot, and a redemption answered anywhere — here, or in the dashboard
— leaves it, because Twitch says so over EventSub either way.

## Polls

A poll runs for a minute or two and the whole point is watching the
numbers move, so the running total belongs on the screen the broadcaster
is already reading: the window keeps a bar above the message line, with
every choice, its share and its count, and the two ways to stop it —
with the result shown, or hidden altogether. **Moderate → Poll…** asks
for the question, up to five choices and how long voting stays open.

The terminal says it rather than drawing it: a line when a poll starts
and another when it ends, `/poll` for how it stands right now, and
`/poll <seconds> <question> | <choice> | <choice>` to start one. The
counts arrive several times a second while people vote, and a line each
would bury the chat the poll is running in.

Joining one's own channel asks Twitch whether a poll is already running,
because EventSub says nothing until the next vote: a client started in
the middle of one would otherwise show nothing over a channel that is
mid-vote. The engine keeps what it finds in the channel's state and in
every snapshot, so a frontend that attaches mid-poll sees what everyone
else is looking at. It comes from `channel.poll.begin/progress/end` over EventSub, but
starting and stopping also answer with the poll itself — the subscription
may have been refused, while the answer to a request is certain.

Twitch counts votes paid for with channel points separately as well, but
inside the same total; adding them would count them twice.

## Predictions

A prediction is a poll with one more state and much higher stakes:
betting closes, and then somebody has to say who was right. Twitch gives
the broadcaster a day, and cancels it for them if they forget — which is
exactly why doing it from the chat window beats the dashboard.

The window keeps a bar like the poll's, with how many people took each
side and how many channel points they put on it. While betting is open
it offers **Close betting** and **Call off**; once it is locked the
heading says so and every outcome grows its own **Pay out** button, so
the decision is one click away from the numbers it is made from. The
terminal has `/prediction <seconds> <question> | <outcome> | <outcome>`,
`/pred` for the standing, `/lock`, `/payout <number>` and
`/cancelprediction`. The outcomes are numbered because `/payout` takes
that number: nobody is going to type a Twitch id while the chat waits.

Joining asks for one already running, as it does for a poll — and here it
matters more, because a locked prediction may have been waiting since
before this client started. Unlike a poll, the engine keeps a prediction
while it is locked — that is
the state nobody may lose sight of — and lets it go only once it is paid
out or called off.

## The user card

`/user <name>`, or **Kdo to je?** in a message's menu, asks the engine
what is known about one person in this channel: when the account was
made, since when they follow, and whether they are already banned here.
The last two are answered only where this account may ask — a moderator
of that channel — and are simply absent otherwise rather than guessed at.

Whether they subscribe, and at what tier, comes from the channel's own
list of subscribers — **Get Broadcaster Subscriptions** filtered to one
person, not **Check User Subscription**, which answers only "do *I*
subscribe to this channel" and is no use in a card about somebody else.
A refusal is not an answer. In a channel this account neither owns nor
moderates, Twitch answers both questions with "no permission for the
broadcaster", and the card used to print that as "not following" and "not
subscribed" — statements about a person that nobody had made. It says
Twitch would not say.

The window shows it as a card, and adds the one thing Twitch cannot
answer: what that person has actually been saying, taken from the chat
already on screen. Twitch's timestamps carry the hour and the time zone;
a card shows the day, which is what is read. The two commands that
usually follow — a timeout and a ban — are on the card, so the decision
and the acting on it are in one place.

## The lists

`/mods`, `/vips` and `/banned` say who moderates the channel, who is a
VIP there, and who is banned or in a timeout — the `/mods` and `/vips` of
the old chat, plus the one the web interface buries in a settings page.
A timeout says when it runs out and a ban says nothing, which is the
difference worth seeing.

Twitch takes all three only from the channel's own login (`broadcaster_id`
must match the token), so the engine checks that itself and says so, as it
does for the title. They need no permission this client did not already
ask for, and pagination is followed to four hundred names.

The engine sends the names as data rather than as a sentence, and each
frontend does what it can with them: the terminal says them in a line, as
it always did, and the window opens a list. Four hundred names do not fit
in a line of chat, and a name in a list is usually the beginning of a
question rather than the end of one — so a row opens that person's card,
and somebody banned can be let back in from there.

## The moderating panel

A third column, off by default and turned on by the **Review** button.
What is waiting for a decision — AutoMod's catches and the unban requests
— sits at the top with the two answers beside each, and the channel's
moderation log runs underneath. Both are per channel; a moderator of three
channels has three queues and mixing them would be worse than useless.

It exists because the chat is the wrong place for either when a channel
is busy: a held message is read while it scrolls away, and the log is
drowned by the conversation it is about. Both still go into the chat as
well, which is enough in a quiet channel and means nothing is lost when
the panel is off.

The panel knows nothing of the engine: it is handed a closure that
answers, and it takes an item off the list only when the engine says it
was decided — by us or by another moderator. The queue takes the height
and the log keeps a strip at the bottom, because what is waiting is what
has to be read; the divider drags either way.

## Hotkeys

`[hotkeys] bindings`, one `KEY what to do` a line, read in
`liveside-core` (`hotkeys.rs`) like custom commands and checked the same
way. What follows the key is read as a custom command's body, so it has
the same replacements; `$1` is the user of the selected message, which
is Chatty's hotkey context. The frontend turns a key press into a `Key`
(the terminal from crossterm, the window from GDK, caught on the window
before its own keys) and runs the line through the same path a typed
one takes, custom commands included; the input line is not touched.

A binding needs F1–F12, or Ctrl or Alt with a letter or digit, so it
can never take a key away from typing; it does take a built-in key
over, except F2 (the settings in both frontends: a binding there could
lock someone out of the place to undo it) and Ctrl-C and Ctrl-Q. Keys
held outside the window would need the desktop's help on Wayland, so
there are none. On a Mac the window takes Cmd for Ctrl, in the built-in
keys and in bindings alike: GTK reports Cmd as Meta there, and Cmd is
what a Mac user presses for Find (Ctrl still works, as before). Ctrl-Q,
Cmd-Q there, quits by closing the main window.

## Custom commands

Chatty's: a list of `/name what it does`, kept under `[commands] custom`.
The syntax is read in `liveside-core` (`commands.rs`), so a line that
cannot be read stops the settings file like a broken rule does and the
settings screen will not save it; what cannot be done here (Chatty's
other functions, its context-menu values) is refused with the reason.

They are expanded by the frontend, first thing when a line is typed,
because that is where a typed line exists: the engine only ever sees
the result, as if it had been typed. The frontend supplies what the
values name: the channel on screen, the joined ones, the account, and
the stream's viewers and uptime from the stats it has and its title and
category from the last change the viewer history marked. The result is
not expanded again, as in Chatty, so commands cannot call each other or
loop. A missing required value refuses the command and says which.

`//` keeps meaning "send this line starting with `/`", which it did
before custom commands, so Chatty's anonymous `//` commands are not
offered.

## The user's own bots

Decided 2026-10-19, from the owner's weather bot (`!pocasi [město]`,
answered from wttr.in), which talked IRC itself with a token pasted
into the script. A bot stays a program of its own, in any language, but
it does not talk to Twitch: the engine starts it and is its only way
out. What a bot has to do is in `docs/bots.md`; this is why.

- **A child process speaking JSON lines** on its standard input and
  output: the messages of its channels in, `say` and `reply` out.
  Anything that reads and writes lines can be a bot, and none of them
  needs a token, a login, a reconnect or Twitch's rate limits; the
  client already has all of that. The token is never handed to a bot,
  so a script cannot leak it or use scopes it was not meant to; a way
  to ask for it, per bot, may come later if a bot needs an API the
  engine does not wrap.
- **Run by the engine**, not a window: with a daemon, bots go on while
  no window is open, and both frontends list, start and stop them with
  commands (`StartBot`, `StopBot`, `SetBots`) and see their state and
  output as events (`BotState`, `BotLog`) and in the snapshot.
- **Defined in `config.toml`** under `[[bots]]` (a name, the command as
  a list of words, a working directory, channels, whether it starts
  with the engine), so the file can be edited by hand as well as from
  the window's list; a list saved there is handed to the running engine
  with `SetBots`.
- **The account's own voice.** A bot speaks as the logged-in account; a
  second account for bots would be a second login, and the protocol
  would not change for it.
- **What a bot is shown:** chat messages in its channels (all joined
  channels if it names none), as said rather than as a filter changed
  them, but not history, and by default not what the ignore rules
  hide, so ignoring somebody also keeps them from a bot. What the user
  types as the account it hears, since the streamer tries their own bot
  first; what a bot said, coming back as the account's, it does not
  (another bot may, if set to), or it would answer itself in a loop.
  Such a line is known by its channel
  and text within 30 seconds of being sent, since the backend's answer
  to a send carries no message id to know it by.
- **What a bot may do:** say something, or reply to a message, in one of
  its channels. Nothing else for now; moderation would be a permission
  given to one bot. At most `rate` lines in 30 seconds per bot, 20 unless
  set otherwise, Twitch's own limit for an account that is not a
  moderator (100 where it is one or owns the channel, the most allowed);
  more are dropped and said in its log rather than queued, since a bot
  that floods is broken.
- **The guards are each bot's settings** (the owner's choice): its
  channels, `rate`, `stop_after`, and whether it hears other bots
  (`hear_bots`) and ignored people (`hear_ignored`), both off unless
  set. Hearing itself is not a setting: a bot whose answer starts with
  its own command would loop, and nothing is gained by allowing it.
- **Stopping** closes its input, the sign to finish, and kills it after
  `stop_after` seconds (2 unless set) if it has not. A bot that exits by itself is shown as
  ended, with its exit code; it is not restarted, since one that keeps
  failing would only fail again (the owner's choice). Its standard error,
  and any output line that is not understood, go to its log, the last
  200 lines of which are kept.
- **One copy per machine.** A daemon and a window running its own engine
  would each start the bot and answer every command twice, so a running
  bot holds a lock, `$XDG_RUNTIME_DIR/liveside/bots/<name>.lock`; the
  second engine says it is already running elsewhere, tries the lock again
  every five seconds and takes the bot over once the first lets it go
  (closed, or stopped it), so closing the window does not leave a
  terminal client without the bot. Stopping the bot in the second engine
  ends the wait.

## The viewer graph

Twitch says how many watch only as a number now, so the history is the
client's own: the engine notes each minute's count (the answer to the
same question that says who streams, so it costs nothing more) in
`ViewerHistory`, together with each change of title or category it sees
in the channel, and starts again when a stream with another start time
comes. It keeps a day of samples per channel and hands them over in the
snapshot, so a window opened later, or attached to a daemon, draws the
stream from where the engine began watching it. Frontends add to their
copy as `StreamStats` and the channel's `event.channel_update` lines
arrive, with the same type and code, so they cannot drift apart.

The window draws it with cairo in a `DrawingArea`: no plotting library
for one line and a few labels. The terminal draws it with ratatui's
`Chart` in braille, whose two-by-four dots per character are fine enough
for a line; `/viewers` puts it over the chat until Esc, the changes as
lines of their own colour and numbered in a list under it.

## The user list

Two sources, chosen by the `user_list` setting.

**`active` (the default)** is built in the frontend from the lines it
already has: whoever has talked, ranked broadcaster, moderator, VIP,
subscriber, then the rest, alphabetically within a rank. It costs nothing
and works in every channel. Someone whose last message has scrolled out of
the buffer drops off the list, so it stays "who is active" rather than
"who was ever here".

**`irc`** also opens Twitch's old IRC interface (`liveside_twitch::irc`, over
WebSocket so it needs no second TLS stack) purely for membership. Helix
tells only moderators who is present (Get Chatters needs the
`moderator:read:chatters` scope *and* moderator status) and EventSub has no
membership events, so IRC is the only way an ordinary viewer can see people
who never say anything.

Measured on 2026-09-24: an **anonymous** IRC login is enough, so this asks
for no additional scope and sends no token over that connection. Joining a
channel is answered with its member list — 666 names for a channel with
about 1500 viewers — sent as many `353` pieces and ended by `366`, which is
why the pieces are assembled before the list is published. JOIN and PART
follow for small channels; busy ones get few or none, so the list is asked
for again every five minutes (re-joining is answered with a fresh list).

The demo backend invents a membership of its own when the setting says
`irc`: a few dozen `lurker_NNN` who never speak, and the odd join or part
as the chat runs. Without it the switch could only be tried against real
Twitch, and neither frontend's handling of membership could be tested at
all.

Membership reaches frontends as `Chatters` (the whole list) plus
`ChatterJoined` / `ChatterParted`, and sits in each channel's snapshot. The
frontend merges the two sources: someone who has talked keeps their badges
and display name, and the rest are listed by login. Both frontends do the
merging with the same code (`liveside_client::chatters`), so the two windows
never disagree about who is in a channel; each only has to say which
messages it still holds.

## Settings

One file, `$XDG_CONFIG_HOME/liveside/config.toml`, read by the daemon and by a
frontend running its own engine; command-line options override it. A
missing file means defaults, but a broken one (bad syntax, or a key that
does not exist) stops the program: a setting that silently does not apply
is harder to notice than an error at startup. `Config` turns into
`EngineConfig` and `TwitchConfig`, so there is one place where a setting
becomes behaviour.

The error says where and what: the parser's own message already names the
line and points at the text, and `ConfigError` adds the file, the
consequence ("none of its settings were applied") and, for an unknown key,
the closest valid name. `--check-config` prints the same report, or the
settings as they will be used, without starting a client or a daemon; the
daemon checks before it binds its socket.

The settings screen (F2) is a list of typed rows — switch, number, list of
words — rather than a screen written per setting, so the GUI can render the
same rows as checkboxes and fields and cannot end up offering a different
set of settings. The one exception is a setting only one frontend can obey,
such as whether emotes move: `Settings::new` is told which frontend it is
building for and leaves that row out of the other, which keeps the file's
value untouched rather than writing it back from a row that was never
shown. Rows the engine only reads at startup say `(restart)`:
reconfiguring a running engine would need protocol commands, which is not
built yet, and an edit that appears to do nothing would be worse.

## Not showing the same message twice

The engine drops a message whose id it has shown recently, and history is
filtered against what a channel already holds. Two engines on one machine
(a daemon and a frontend with its own engine) append to the same chat log,
so the file can hold a message twice and restored history can overlap what
is about to arrive live. Seen exactly that way, as doubled lines on screen.

## History, and privacy as a feature

History is **local only**: the engine writes the chat it sees to
`$XDG_DATA_HOME/liveside/history/<channel>.jsonl` (0600 in a 0700 directory) and
restores the tail of that file when the channel is joined again. Nothing is
fetched from anywhere, so a channel joined for the first time starts empty.
An earlier version loaded the missed messages from Robotty's
recent-messages relay, as other clients do; that was removed, because it
told a third party which channels the user reads. See `crates/core/src/history.rs`.

That decision generalizes: **anything that sends data anywhere must be
visible and switchable.** Rather than trusting documentation to stay
truthful, the engine reports its own behaviour: backends emit
`Event::DataFlows` describing what they contact and why, the engine adds
its own (the chat log) and includes the list in every snapshot, and both
frontends show it on `/privacy` — the terminal as lines in the status
buffer, the window as a table it also reaches from **Settings → My data**,
beside the switches that report names. The list is built from the settings
that drive the behaviour, so it cannot drift from the code.
`docs/privacy.md` is the prose version for people who have not installed
anything yet.

Current flows: Twitch itself (not optional, this is a Twitch client), the
three emote providers (`--no-third-party-emotes`), the token file, the
chat log (`--no-history`), VirusTotal (`security.virustotal`), and the
sites a followed link leads through (`security.resolver`). There is no telemetry.

The engine also names its backend (`Event::Backend`, and in every
snapshot), and frontends mark anything that is not `twitch`. The demo
backend joins any channel name and invents chat that looks exactly like
the real thing, which is confusing precisely when it matters; the terminal
frontend therefore shows a red `DEMO` badge in its header.

## Images and emotes

The engine resolves emotes to provider + id; it does not ship image bytes
or URLs in events. Frontends that render images (GUI; TUI only if we ever
add terminal graphics) build the URL with `liveside_twitch::emotes::image_url`
and cache the files themselves. The TUI shows emote codes as styled text.

Twitch reports only its own emotes, as message fragments. 7TV, BTTV and
FFZ emotes arrive as plain text, so the backend fetches each provider's
global set at login and each channel's set on join, then rewrites whole
words into emote fragments. Precedence: channel before global, and within
each, 7TV before BTTV before FFZ. A provider that is down is skipped, and
chat works without it.

Emotes are fetched in the background, never on the path to chat. They were
awaited during login and during a join, so a provider that stopped
answering kept the user out of their own chat; with `reqwest`'s default of
no timeout at all, "slow" meant "for ever". The HTTP client now has
connect and request timeouts, and emote sets are applied when they arrive,
so the first seconds of a channel may show codes as text.

Known gaps: sets are never refreshed while running (providers do add
emotes); BTTV does not mark zero-width emotes in its API (other clients
hard-code a list of ids); FFZ "modifier" emotes are all treated as
zero-width, which is an approximation. A provider that does not know a
channel answers 404, which counts as "no emotes", not as a problem.

## Links

Designed 2026-10-13. Built so far: finding links, the three ways of
opening them in the window and (OSC 8) in the terminal, the Security
section, VirusTotal and the *local* resolver in both frontends. Not yet:
the *service* resolver, whose service does not exist yet. Chat is where moderators get
phished
and where streamers get sent IP loggers, so a link is treated as
something to be careful with rather than as decoration.

**Finding them.** The engine finds URLs and fills
`Fragment::Link`, so both frontends get the same fragments. Only `http`
and `https`. An address is always shown whole, never shortened: in chat
the text *is* the target, and nothing can be spoofed until we start
cutting it.

**Opening them.** `security.links = "ask" | "open" | "off"`, default
`ask`. In the window *ask* opens a confirmation with the whole address
and its host emphasized; a host with anything but ASCII in it is shown
in punycode as well (`xn--…`), which makes a look-alike letter visible.
*open* hands the address to the default browser at once; *off* leaves it
text to copy. The terminal cannot ask, so there *open* emits OSC 8 and
the terminal decides, and *ask* and *off* emit nothing; the settings
row says so in the terminal's own words. A terminal that finds addresses
in text by itself does so whatever this says.

OSC 8 is not written into ratatui's cells, where the escape sequences
would count as text and push the rest of the row aside. The renderer
carries each span's link through wrapping and reports where the pieces
of every link landed; after ratatui has drawn, those cells are written
once more over themselves, same characters and style, inside the OSC 8
sequence (`crates/tui/src/hyperlink.rs`). The pieces of one wrapped link
share an `id`, so the terminal treats them as one link. A deleted
message's link is not made clickable, and a change of the setting
clears the screen, since cells ratatui thinks unchanged would otherwise
stay links. Checked in kitty with its own URL detection off: a click on
either half of a wrapped link opened the whole address.

**Where a link leads.** Checking is done by the engine (so a daemon does
it once for every frontend), asked for by a command and answered by an
event, and the answer is kept for the session per address, so it is
shown beside every message carrying the same link; one link can have an
answer from VirusTotal and one from a resolver, written side by side.
Three ways, chosen with `security.resolver = "off" | "service" |
"local"`, default `off` (`service` is not accepted until it is built):

- *service*: a web service at `security.resolver_url` follows the
  redirects, so the reader's address never reaches the link's owner.
  Proposed contract: `POST` with `{"url": …}`, answer
  `{"final": …, "hops": [{"url": …, "status": …}], "status": …}`. The
  service is planned and does not exist yet; the client is written
  against the contract.
- *local*: the engine follows the redirects itself. The settings say
  plainly that every host on the way learns the reader's IP address —
  which a shortener built as an IP logger is made for. It is offered
  because that is not always what matters (on a public connection the
  address says little), because what it does protect against is the
  browser rendering the page, and because it works when the service is
  down or the connection is poor. Rules: `HEAD`, falling back to `GET`
  for servers that refuse it; only the status line and `Location` are
  read and the body never is; no cookies, no referrer; at most 10 hops
  and 5 seconds a hop; a `Location` that is not `http`/`https` ends it;
  and any hop whose address is loopback, private, link-local,
  unique-local or multicast is refused, checked on the address actually
  connected to so that DNS cannot hand over a different one in between:
  the HTTP client resolves names through a resolver of ours that drops
  such addresses, and an address written in the link is checked before
  any request. The system proxy is not used, since it would do the
  resolving instead (`crates/core/src/redirects.rs`).
  A link in chat must not be able to make the reader's machine ask the
  router. Redirects done by JavaScript or `meta refresh` are not seen,
  and the answer says that the page it ends at may lead further.
- *VirusTotal* (below) also reports where a link ends.

**VirusTotal.** `security.virustotal = true | false`, default `false`.
The API key is the reader's own and is kept beside the Twitch token in
its own file, `$XDG_CONFIG_HOME/liveside/virustotal.key` (0600), not in
`config.toml`, which people copy and paste. Two steps:

1. Look up an existing report: `GET /api/v3/urls/{id}`, the id being
   the URL in unpadded base64url. Nothing is submitted.
2. If there is none, submit it for a scan (`POST /api/v3/urls`) — only
   when the reader asks for that link, since a submitted URL becomes
   visible to VirusTotal's paying customers. For a link posted in a
   public chat that is little, but it is said where the action is.

Shown: how many engines call it malicious or suspicious out of how many
(`last_analysis_stats`), when it was analysed, and `last_final_url` with
`redirection_chain`, since VirusTotal follows the redirects on its own
servers. The public API allows 4 requests a minute and 500 a day; the
engine queues within that and says when the day's quota is gone.
Checked on 2026-10-13 in VirusTotal's documentation: the public API
"must not be used in commercial products or services". So this stays a
bring-your-own-key feature; a paid edition would need the Premium API or
leave it out.

**Where the actions are.** In the window, the right-click menu on a link,
in a channel or a whisper: open (as the setting says), copy the address,
*Where does it go?* (while a resolver is chosen), *Check with VirusTotal* —
which becomes *Send to VirusTotal for a scan* once VirusTotal has said it
does not know the link. The answer is written after the link in the chat line
(`→ https://… · VirusTotal 0/94`) rather than in a dialog. In the
terminal, `/link` acts on the selected message (Ctrl-Up/Down), or on the
newest message with a link when none is selected: `/link virustotal`,
`/link scan`, `/link where`, with a number when the message has more than
one link.

**Settings.** These are the rows of the *Security* section. Each row
names its section, and a frontend writes a heading where the section
changes — but only when there is more than one, since a heading over the
whole list says nothing. The key is a row of its own type, a secret:
the stored key is never shown again, only whether there is one, and what
is typed (masked) replaces it on saving, into its own file. Every
service here is a data flow on `/privacy` while it is switched on, and
the local resolver's says that the link's hosts see the reader's
address.

## Rules

Highlighting, ignoring and filtering share one rule language, Chatty's
(`crates/core/src/rules.rs`): one rule per line, conditions as prefixes
and the text to find last, `chan:harborlight w:rust`. Chatty's syntax rather
than one of our own, because people coming from Chatty have lists in it
and a rule that reads the same in both is one nobody has to translate.
It is written from Chatty's help page, not its code.

**Where.** The engine applies the rules to every channel line before it
keeps or publishes it, so the window, the terminal and a daemon's
frontends see the same: the `highlighted` flag with the rule's colours
and its `config:silent` / `config:!notify`, a filtered text, or nothing
at all for an ignored line. History restored from the log goes through
the rules as they are now. The log itself keeps the line as it was said,
an ignored one included: the log is the record of the chat, and rules
change. Saving the settings sends the lists to the engine
(`Command::SetRules`), which applies them to what arrives from then on;
what is on screen is not redone.

**Highlight.** Rules are tried in order and the first that matches
decides the colours. The account's name and the highlight words still
light a line up as before, in the default colours. The account's own
lines are never lit up, hidden or filtered.

**Ignore.** A matching line is not shown and not kept in the scrollback;
every 30 s a channel that hid any says how many (`ignore.hidden`), so it
is clear the filter is at work without a line for each message. A
highlight rule with `config:!ignore` lets its line through.

**What lit up and what was hidden.** The engine keeps the last 500 lines
of each, from every channel, apart from the channels: a highlight that
scrolled away is still listed, and an ignored line is not anywhere else.
They are in the snapshot, so a frontend attached to a daemon later lists
the same; an ignored line reaches a frontend only as `Event::Ignored`,
never as a channel line. The window shows both in a window of its own,
each line with its channel in front; the terminal opens them as buffers
(`/highlights`, `/ignored`) that `/close` closes again. Ignoring
somebody from a message's menu, or with `/ignore name`, adds the rule
`user:name` to the ignore list and saves it at once; `/unignore name`
takes that rule back. It lives in the settings rather than in the engine
because the settings file belongs to the frontend that edits it.

**Filter.** The first filter rule that matches replaces what its text
finds with `..`, or with `replacement:`. Only in the text people wrote:
emotes, mentions and links stay whole, which keeps a link from being
half replaced into something that still looks clickable.

**Info lines.** `config:info` and `config:any` reach subscriptions, raids,
announcements and the notices the engine writes. Their text is matched
as the engine has it, which is English: the frontend translates a notice
when it draws it, after the rules are done.

**The address book.** Names, of people or of `#channels`, each with
categories, kept in the settings file under `[addressbook]` as Chatty
keeps them in its own file. Rules match on them with `cat:` (the sender
is in at least one of the categories), `!cat:` (not in all of them:
Chatty's `!cat:a,b` fails only for someone in both), and `chanCat:`,
`!chanCat:`, `chanCat2:` for the channel, found as `#channel` and, for
`chanCat2:`, also as `channel`. `/join [category]` joins every channel
in one, `[category #]` only `#channel` names and `[category !#]` only
people's. The engine holds the book and looks the categories up as a
line is judged, so the rules themselves do not change with it; it comes
with the rules in `Command::SetRules`, and the same splitting of a
`/join` list (`liveside_proto::join_targets`) is used by the engine and
the frontends, so `[category #]` stays one target wherever it is typed.
It is edited in the settings (a name and its categories a line) and with
Chatty's `/ab` commands, which save at once; Chatty's other name for
them, `/users`, is the user list here. Not done: Chatty's `[*]`
favourites and `live` in a join, mod commands for the book, the import
file, and user colours or badges by category.

**Repeated messages.** Each channel remembers what each person said in
the last `[repeated] minutes` (their last 20 messages), and a new
message is compared with those: without case or whitespace, by edit
distance as a share of the longer text, cut to its first 300 characters
so a wall of text costs no more than a line. One alike enough
(`similarity`, in percent) counts; when the count, this one included,
reaches `times`, the line carries it (`LineFlags::repeated`), the
frontends write "(×N)" after it, and rules see it as
`config:repeatedmsg[|N]`. `only` is a rule a message must match to be
counted at all (`!status:M` leaves moderators alone). Live messages
only: history read again on joining would count everything twice. The
memory is pruned with the ignore count, every 30 s. The settings travel
with the rules in `SetRules`, so a change applies at once; `times = 0`
turns it off, and then the rules never see a repetition, as in Chatty.

**Differences from Chatty.** Regular expressions are Rust's (`regex`),
which have no look-around and no backreferences; a rule using them is
refused with the parser's reason, not quietly turned into something else.
Chatty prefixes that need what this client does not have (`msgs:`,
`if:`, message tags, `to:`, `ncat:`, presets) are refused the same way, as is `status:r` without a bot list. A rule that
cannot be read stops the settings file like any other mistake in it, and
the settings screen will not save one; the terminal checks each rule as
it is typed.

## Notifications and sounds

**What is worth a notification.** Five things, each with its own switch:

- a message that mentions the account or matches the highlight words
  (the engine's `highlighted` flag), never one of our own and never one
  restored from history;
- a whisper from somebody else;
- a joined channel that starts streaming — a change from offline to live,
  not the state a join or a reconnection reports, so starting the client
  in the middle of a stream says nothing;
- a followed channel, one not joined, that starts streaming (below);
- moderation: a message held by AutoMod or the blocked terms, and an
  unban request. Only those newer than the frontend itself: a join lists
  the requests already waiting, and they are not news.

**Followed channels.** EventSub would say the moment a channel goes live,
but each `stream.online` subscription is one per channel and costs from
the socket's budget of ten for channels that have not authorized this
client — the budget joined channels already strain. So the engine asks
**Get Followed Streams** once a minute instead, while the token has
`user:read:follows`, and sends `FollowedStreams`: who of the followed
streams now, and who of them began since the last look. The first look,
made right after logging in, is how things stand and names nobody; a
look that fails changes nothing, so a minute without network is not every
followed channel ending and then starting again. A joined channel is left
out of the news, since it says so itself with `Stream`. Several starting
at one look are one notification. Both frontends also say each start in
the status view, with a `/join` to get there, and the window keeps an open
list of followed channels current from the same event. It plays the
going-live sound; a sound of its own did not seem worth another file
setting.

**Where.** The frontend, not the engine, like Streamlink: a daemon may run
on a machine with no screen and no speakers, and two frontends attached to
one engine may sit at two desks. What to notify about is decided in
`liveside-client` (`notify`), from the events every frontend gets anyway,
so the two cannot disagree; how it reaches the reader is up to each.

**When.** By default only when the reader cannot see it already: the
window does not have the focus, or another channel or whisper thread is on
screen. The alternatives are *when the window is not focused* and
*always*. A terminal says whether it has the focus only when it supports
focus reporting; one that does not is taken to have it, so there only
other buffers notify.

**How.** A desktop notification through the freedesktop D-Bus service,
spoken to directly with `zbus` (pure Rust, so nothing to link) over one
connection that stays open. One notification per kind and channel
replaces the previous one rather than stacking up, so a chat that keeps
naming the reader shows one bubble with the latest message. The one
connection is what makes that work on GNOME, which lets only the
connection that raised a notification replace it; `notify-rust`, used
before, opened a connection for each, so there every replacement was
refused (found 2026-10-23). It replaces only while the bubble is on
screen (for `close_after`, or ten seconds when the desktop decides): one
that has gone is closed and a new one raised, since Plasma, which keeps
an expired notification in its list, updates it there without showing
anything, so every notification of a kind after the first went unseen
(found 2026-10-23). Closing the old one keeps that list from filling up.
Without a session bus (over SSH, say) there is simply none, said once in
the status view.

GTK's own `gio::Notification` was tried and set aside (2026-10-23): a
click reaches the program only by the name `net.jifox.liveside.Liveside`, which
an instance of the window does not own (every start of the window is a
program of its own); GNOME, and the portal on labwc, then lose the click.
GNOME also drops a notification from a program it has no `.desktop` file
for, without an error, so a build run from the source tree would show
none. It would not have served the terminal either. A Mac has no
freedesktop service, so a build there shows no notifications; that is
for whoever makes the Mac build (`roadmap.md`, *macOS*).

Each notification says how long it stays, 10 seconds unless set
otherwise (`close_after`; 0 leaves it to the desktop). Leaving it to the
desktop was the first choice and it misbehaved: mako's default is to
keep a notification until it is clicked away, so on the development
machine every one stayed (seen 2026-10-19). Said by the client, it
behaves the same on every desktop.

Clicking a notification goes where it happened: the channel or whisper
thread is opened, a followed channel that is not joined is joined and
opened when it arrives, and the one about several followed channels opens
their list. The notification carries the `default` action, which the
specification makes the click on the notification itself; one thread
hears the service's signals on the same connection and hands the place
to the frontend's own loop. The window also asks to come to the front,
with the activation token the service sends just before the click, so a
desktop that guards against focus stealing lets it. On X11 no service
sends one, and GTK presents the window with the time it was last used,
older than the window in front: xfwm4 then raised it but left the
keyboard where it was. There the window takes the X server's present
time instead, as a startup ID of the form `_TIME<time>`, which GTK reads
as the moment of the click (`gdk4-x11`, Linux only). The keyboard goes to
the message line, as a mention or a whisper is clicked to answer it. A
terminal cannot be raised from inside, so there only the buffer changes.

The window plays a sound: by default one from the freedesktop sound theme
(`message-new-instant`, `message`, `service-login`, `dialog-warning`),
looked up in `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`, so nothing is shipped
and no sound file needs a licence of its own; each kind can name a file
of its own instead. A sound of a kind plays at most once in a few
seconds, however many messages arrive.

The sound is Liveside's alone: every notification asks the desktop for
none of its own (`suppress-sound`). Cinnamon plays one with every
notification otherwise, so each came with two sounds, and with one
where the sound was switched off or a rule says `config:silent` (seen as
two playback streams on Linux Mint, 2026-10-26).

The sound is played by a program in a process of its own, `pw-play`
(PipeWire) or else `paplay` (PulseAudio), one of which a desktop with
sound has; on a Mac, `afplay`. A Mac has no freedesktop sound theme
either, so its own alert sounds in `/System/Library/Sounds` stand in
(Glass, Ping, Hero, Basso). GTK's own media backend was tried first and dropped: it set up
OpenGL for video it would never show, and in a machine without working
OpenGL that brought the whole window down at the second sound (seen,
`SIGSEGV` in the renderer). A player that fails, or crashes, cannot take
the chat with it, and an audio library linked in would be one more thing
that could. Neither a missing player nor a file that will not play is
silent about it: the status view says so, once.

The terminal rings its bell instead: that works over SSH, in tmux, and
everywhere else a terminal goes, and it is the terminal's business how it
sounds. The sound files are not offered there.

**Settings.** A *Notifications* section: when, desktop notifications on or
off and how long they stay, sound (the bell in the terminal) on or off,
the five kinds, and in
the window a file for each kind. All of it is read as the event arrives,
so none needs a restart.

## Packages

A Flatpak is the package for every distribution (`docs/roadmap.md`),
built by `packaging/flatpak/`. Its sandbox holds only the GNOME runtime's
programs and none of the user's files, which runs against what three
features are: Streamlink with a player, the user's own bots, and a sound
file of the user's own. Those run on the host through `flatpak-spawn
--host` (`liveside_core::host`), which the manifest allows by letting the
app talk to `org.freedesktop.Flatpak`. That lets the app run anything as
the user, so the sandbox protects little; it was chosen on 2026-10-22 over
a strict sandbox, which would have meant bundling Streamlink and a player
and confining bots to the runtime's Python, since a native package
reaches as far anyway. Liveside is not on Flathub yet (`roadmap.md`,
*Before the first public version*); Flathub accepts the permission only
with a reason given, and if it does not, the Flatpak stays a bundle of
our own.

What stays inside the sandbox: the window; links, through the portal;
notifications (`org.freedesktop.Notifications`); sound, through
PulseAudio's socket, since pw-play cannot reach PipeWire there and so
paplay plays; and the freedesktop sound theme, bundled because the
system's is out of reach. Settings, token and history are the Flatpak's
own, under `~/.var/app/net.jifox.liveside.Liveside/`, apart from a native
install's. Two instances of the Flatpak share `$XDG_RUNTIME_DIR` and so
the bots' locks; a Flatpak and a native install do not see each other's.

## Dependencies

Quality and functionality first. Copyleft dependencies are acceptable when
they are the best option, even though that rules out a proprietary edition;
every such dependency must be listed here with its license.

| Dependency | License | Why |
|---|---|---|
| `serde` | MIT OR Apache-2.0 | (De)serialization of the protocol |
| `serde_json` | MIT OR Apache-2.0 | Socket wire format (JSON lines) |
| `tokio` | MIT | Async runtime, channels, sockets, signals |
| `ratatui` (+ `crossterm`) | MIT | Terminal frontend |
| `terminal-colorsaurus` | MIT OR Apache-2.0 | Asks the terminal whether its background is light, so names are corrected the right way |
| `gtk4` (+ `glib`) | MIT; the C libraries under them LGPL: GTK 4 and GLib 2.1-or-later, Pango 2.0-or-later, Cairo 2.1-only OR MPL-1.1; linked dynamically | Graphical frontend |
| `async-channel` | Apache-2.0 OR MIT | Engine messages into the GTK main loop |
| `image` | MIT OR Apache-2.0 | Decoding emote and badge images (PNG, GIF, WebP) |
| `jiff` | Unlicense OR MIT | Local time for timestamps |
| `unicode-width` | MIT OR Apache-2.0 | Column widths for wrapping in the TUI |
| `reqwest` | MIT OR Apache-2.0 | HTTP for OAuth and Helix (rustls, no OpenSSL) |
| `tokio-tungstenite` | MIT | EventSub WebSocket (rustls) |
| `futures-util` | MIT OR Apache-2.0 | Stream handling for the WebSocket |
| `emojis` | (MIT OR Apache-2.0) AND Unicode-3.0 | `:shortcode:` → emoji when completing |
| `url` | MIT OR Apache-2.0 | Checking links found in chat, hosts in punycode (already there through `reqwest`) |
| `zbus` | MIT | Desktop notifications over D-Bus, from both frontends |
| `gdk4-x11` | MIT | The X server's time, so a clicked notification brings the window the keyboard on X11 (Linux only) |
| `regex` | MIT OR Apache-2.0 | The `reg` family of the rule language; also matches every literal case-insensitively |
| `toml` | MIT OR Apache-2.0 | The settings file, and the interface's text in a file per language |
| freedesktop sound theme (Flatpak only) | GPL-2.0-only, GPL-2.0-or-later, CC-BY-SA-3.0, CC-BY-3.0 (per sound, in its `CREDITS`) | The sounds, which a sandbox cannot take from the system |

## Licensing

**GPL-3.0-or-later**, decided by the owner on 2026-10-27, so that Liveside
stays open source even in a fork: whoever passes on a changed copy passes
on its source under the same terms. Every dependency in the table above
can be combined with it, Apache-2.0 included; the LGPL C libraries under
GTK are linked dynamically.

Outside contributions are taken under the same licence and nothing more,
decided on 2026-10-27: no contributor agreement. So a contribution can
never be licensed otherwise without its author's consent, and a paid
edition with closed parts could hold only code the owner wrote alone.
