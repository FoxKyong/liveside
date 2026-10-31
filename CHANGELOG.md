# Changelog

What changed in each version of Liveside. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/).

## Unreleased

### Added

- Telling the chat about a link: **Check with VirusTotal and tell the
  chat** in a message's menu, and **Tell the chat** in the window that
  asks before opening a link, which says whether the address imitates a
  name with letters from other alphabets. The message starts with "Link
  check:", names the link and whose it was, never as a link itself, and
  has a warning sign in front when something is wrong. One or two of
  VirusTotal's engines alone, which flag even google.com now and then,
  are reported as a likely false alarm rather than as a warning.
- The usage guide says how to watch logged in with Streamlink, so that
  a subscriber gets no ads: Liveside's login cannot be used for that, a
  token from the browser in Streamlink's own settings can.

### Changed

- The channels to open at start are no longer a setting of their own.
  Liveside already opened again the channels open at the end, so joining
  a channel (**Join a channel…**, `/join`) is what keeps it, and leaving
  it (the channel's menu, `/part`) is what drops it. A `channels` line
  from an older settings file starts that list if there is none yet.

### Fixed

- Channels opened again at start show the turning wheel while they are
  joined, like channels joined by hand; before, the window stood without
  them until each was ready.
- Leaving a channel takes it off the list at once. Twitch is told to
  stop sending its events all at once rather than one after another,
  which took ten seconds and more in a channel of one's own.

- Cmd-Q quits on a Mac.
- The message line no longer tells someone logged in to log in.
- The terminal: the time of restored lines stays readable on dark themes
  that are grey rather than black, the ban-evader mark is readable in
  Terminal.app, and the cursor no longer stands on the settings' frame.
- `--check-config` says in plain English from which repeat a message is
  marked.
- A channel opened for the first time after start shows its newest
  line; one whose history came in while another channel was on screen
  stood somewhere in the middle of it.
- Scrolling a form with the mouse wheel no longer stops at a number field
  and changes the number, as the arrow keys no longer did.

## 0.9.0 — 2026-10-29

The first public version: complete, but not yet tried by many people.
1.0 follows once it has been. Since there is no earlier version, this
lists what it can do rather than what changed.

### Frontends

- A graphical frontend (GTK 4) and a terminal one, over one engine and one
  settings file (`~/.config/liveside/config.toml`, also edited on F2).
- The engine can run as a daemon that several frontends attach to over a
  Unix socket, with a systemd user service to start it with the session.
  A window whose daemon goes away greys its channels and attaches again
  when it is back.
- `--demo` runs against a made-up chat, without a Twitch account.

### Chat

- Logging in with a code entered at twitch.tv/activate, which works in a
  terminal and over SSH. Chat is read through EventSub and sent through
  Helix.
- Emotes from Twitch, 7TV, BetterTTV and FrankerFaceZ, animated ones
  included, badges, and completion of names, emotes, commands and emoji
  shortcodes.
- Replies, mentions, searching the chat, whispers in threads of their
  own, Shared Chat, and a local chat log that brings a channel back with
  its recent lines, a line with the date where a new day starts. The
  channels open at the end open again at the next start.
- Highlight, ignore and filter rules in Chatty's syntax, an address book,
  custom commands, hotkeys, and marking what somebody keeps repeating.
- Links open only after their address is confirmed. Where a link leads
  and VirusTotal's view of it can be asked for, each switched on in the
  settings.
- Desktop notifications and sounds for mentions, whispers, channels going
  live and messages held for moderators.
- Streamlink to watch a stream, and a Stream Chat window to capture in a
  broadcast.

### Moderation

- Timeouts, bans, warnings, deleting and clearing, announcements, the room
  modes and the chat delay, moderators and VIPs, pinned messages, Shield
  Mode, blocked terms and suspicious users.
- AutoMod's queue, unban requests and the moderation log, in one Review
  panel in the window.
- Lists of banned users, moderators, VIPs, followers and subscribers, and
  a card for each user.

### For broadcasters

- Polls, predictions, channel point rewards and their redemption queue,
  the channel's title, category and tags, and its schedule.
- Raids, shoutouts, stream markers, commercials and ad breaks, and clips.
- Follows, subscriptions, gifts, bits, Hype Trains, goals and charity
  donations said in the chat, and a graph of viewers over the stream.
- The channels one moderates and the ones one follows, which of them
  stream, and a notification when one starts.

### Your own bots

- Programs that Liveside starts and talks to in JSON lines, needing no
  token or Twitch connection of their own (`docs/bots.md`). A weather bot
  is included as an example.

### Languages

- English and Czech, and machine translations into Spanish, Russian,
  Ukrainian, Esperanto, Greek, Korean, Japanese, Chinese (Simplified and
  Traditional), Thai, Vietnamese and Arabic, which is laid out from the
  right.

### Privacy

- `/privacy` lists what is sent where and what is stored, from the
  settings in use. There is no telemetry.

### Packaging

- A Flatpak manifest, a desktop entry and an icon. The Arch Linux
  package is in the AUR, kept in its own repository.
- Licensed under GPL-3.0-or-later.
