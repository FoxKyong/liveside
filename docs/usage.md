# Using Liveside

How to use both frontends, every setting, and the daemon. The
[README](../README.md) has the short version.

## Starting

```sh
liveside-gui                        # the window
liveside-tui --channel somechannel  # the terminal
```

On the first run, log in: **Log in** in the window, `/login` in the
terminal. Twitch shows a code to enter at twitch.tv/activate, and the
login is kept in `~/.config/liveside/token.json` for next time. `--demo`
runs against a made-up chat instead, without a Twitch account; the
window still fetches emote and badge pictures from Twitch's image server.

Chat you see is written to a local log, and a channel you rejoin comes
back with its recent lines. Nothing is fetched from anywhere for this, so
a channel you join for the first time starts empty.

The channels open when Liveside ends open again at the next start, in the
same order. Joining a channel (**Join a channel…** or `/join`) is what
keeps it, and leaving it (**Leave the channel** in its menu, or `/part`)
is what drops it; `reopen_channels = false` in `[chat]` stops that. The
terminal started with `--channel` opens only the channels it names, and
leaves the list as it was. A `channels = [...]` line from an older
settings file starts the list if there is none yet, and is not read after.

Typed commands (`/join`, `/part`, `/timeout`, `/ban`, …) are read by the
engine, so both frontends have the same ones: `/moderated` for the
channels you moderate, `/followed` for the ones you follow (which of them
stream, and what), `/highlights` and `/ignored` for what lit up and what
the ignore rules hid, `/ignore name` and `/unignore name`, `/ab` for the
address book (`/ab add alice friends`, `/ab info`), `/join [category]`,
`/followerlist` and `/sublist`, `/viewers` for how many watched,
`/stream [quality]` to play the channel with Streamlink, `/privacy` for
what is sent where. **About** in the window's settings, and `--version`
for every program, say which version this is.

## The window

Right-click a message to answer or moderate it. **People** shows who is
active, **Moderate** has the room's settings and the broadcaster's tools
(**Channel…**, **Rewards…**, **Schedule…**), **Review** what AutoMod
holds, unban requests and the moderation log, and **Settings** the
settings. The buttons under the channel list show the channels you
moderate and the ones you follow, each with a Join button, and what lit
up and what was hidden; **Join a channel…** asks for any other. Right-click
a channel to open it in a window of its own or beside the chat, clear it,
or leave it. A message's menu can ignore its author.

A link in a message can be checked with VirusTotal (with your own key,
switched on in the settings), and **Check with VirusTotal and tell the
chat** sends the answer to the chat once VirusTotal has given it. The
window that asks before opening a link has **Tell the chat** too, which
says whether the address imitates a name with letters from other
alphabets and where it really goes. Either message starts with "Link
check:", names the link and who wrote it, and has a warning sign in
front when something is wrong:

    ⚠ Link check: https://pаypal[.]com/login from @someone – the address imitates
    a known name with letters from other alphabets and really goes to xn--pypal-4ve[.]com

The host is written with its dots in brackets, so that the message is
not a link itself, and the message is in the language of the interface.

| Key | Does |
|---|---|
| Tab, Shift-Tab | complete a name, emote, command or `:emoji` shortcode; the one before |
| Up, Down | lines sent before |
| Ctrl-R | answer the newest message; again, the one before it (Esc drops it) |
| Ctrl-F | search the chat on screen: Enter or F3 further back, Shift-Enter forward, Esc closes |
| F4 | a clip |
| Ctrl-Q | quit |

On a Mac, Cmd does what Ctrl does here.

If a pixel or two of earlier text stays behind after switching channels,
it is GTK's GPU renderer missing a row when it redraws ([GTK issue
8340](https://gitlab.gnome.org/GNOME/gtk/-/work_items/8340), still in GTK
4.24, with GL and Vulkan, and only with some fonts). Starting the
window with `GSK_RENDERER=cairo liveside-gui` avoids it; to make that
permanent, put `GSK_RENDERER=cairo` in `~/.config/environment.d/gsk.conf`
(it then applies to every GTK 4 program after the next login).

## The terminal

| Key | Does |
|---|---|
| Enter | send the typed line |
| Tab, Shift-Tab | complete a name, emote, command or `:emoji:`; on an empty line, the next or previous buffer (also Ctrl-N, Ctrl-P) |
| Up, Down | lines sent before |
| Ctrl-Up, Ctrl-Down | pick a message; Ctrl-R answers it (Esc cancels) |
| PageUp, PageDown | scroll a screenful; Esc back to the newest |
| Ctrl-F | search the buffer: Enter or Up older, Down newer, Esc done (the match is picked, so Ctrl-R answers it) |
| F3 or `/users` | the people active in the channel |
| F2 or `/settings` | the settings |
| Ctrl-U, Ctrl-W | clear the line, delete a word |
| Ctrl-L | clear the buffer on screen, for you only (`/clear` is Twitch's, for everyone) |
| Ctrl-C, Ctrl-Q or `/quit` | quit |

## Moderating

Moderator commands work in any channel where you are one: `/timeout`,
`/ban`, `/unban`, `/warn`, `/delete`, `/clear`, `/announce`, `/slow`,
`/followers`, `/subscribers`, `/emoteonly`, `/uniquechat` (each with an
`off` form) and, as the broadcaster, `/mod`, `/unmod`, `/vip`, `/unvip`.
Select a message with **Ctrl-Up/Down** and **Ctrl-T**, **Ctrl-B** or
**Ctrl-D** prepares a timeout, ban or delete for it, to send with Enter;
in the window, right-click the message.

## Settings

Press **F2** (or type `/settings`) to see and change the settings, and `s`
to save them. They live in `~/.config/liveside/config.toml`, which you can also
edit by hand:

```toml
[ui]
language = "auto"         # or a code: en, cs; machine translated: es, ru, uk, eo,
                          # el, ko, ja, zh-Hans, zh-Hant, th, vi, ar. auto follows LANG
theme = "system"          # or "light", "dark": the window's colours, and the
                          # terminal's background; system asks the terminal

[chat]
user_list = "active"      # or "irc": also watch Twitch IRC for who is present
scrollback = 1000
history = true            # local chat log, and history when joining
history_lines = 50
reopen_channels = true    # open again at start the channels open at the end
third_party_emotes = true # 7TV, BetterTTV, FrankerFaceZ

[highlight]
words = ["rust"]          # besides your own name, which always highlights
rules = [                 # Chatty's rule language, one rule per item
  "w:rust",
  "chan:harborlight color:white bgcolor:#8b0000 user:kamarad",
  "config:info subscribed",
]

[ignore]
rules = ["user:nightbot", "!status:smv startw:!bet"]   # hidden, counted every 30 s

[filter]
rules = ["replacement:[…] regwi:(lol)+"]               # replaces what it finds

[repeated]                # "(×3)" after what somebody keeps saying
times = 2                 # from the second time; 0 turns it off
similarity = 90           # % alike, ignoring case and spaces
minutes = 60
only = "!status:M"        # a rule: leave moderators alone

[hotkeys]                 # a key runs a line as if typed
bindings = [
  "F5 /info",                                           # a custom command
  "Ctrl+Shift+B /ban $$1 spam",                         # $1: the selected message's user
]

[commands]                # Chatty's custom commands
custom = [
  "/hi Hello $$1, welcome to $(chan)!",                 # $$1: required
  "/info /echo $(streamstatus), $(streamviewers) watching for $(streamuptime)",
]

[addressbook]             # categories for rules (cat:vip, chanCat:czech)
alice = ["friends", "vip"]                            # and /join [czech]
"#harborlight" = ["czech"]

[streamlink]
quality = "best"          # ▶ in the window, /stream in the terminal
player = ""               # e.g. "mpv"; empty: Streamlink's own choice
arguments = ""            # e.g. "--twitch-low-latency"

[notify]
when = "unseen"           # or "unfocused", "always"
desktop = true            # desktop notifications (D-Bus)
sound = true              # a sound in the window (pw-play, paplay or afplay), the bell in the terminal
mention = true            # mentions and highlight words
whisper = true
live = true               # a joined channel starts streaming
followed = true           # a followed one does (checked every minute)
moderation = true         # held messages and unban requests
mention_sound = ""        # a file of your own; empty: the system's sound theme

[security]
links = "ask"             # or "open", "off": what clicking a link does ("open" makes them clickable in the terminal too)
virustotal = false        # check links you pick with VirusTotal, using your own key
resolver = "off"          # or "local": follow a link's redirects from this machine (sites see your IP)

[twitch]
client_id = ""            # empty: Liveside's own; a fork registers its own at dev.twitch.tv
```

The daemon reads the same file. Command-line options override it, so a flag
is still the way to try something once.

To check the file without starting anything:

```sh
liveside-tui --check-config
```

It prints the settings as they will be used, or, if the file is wrong, the
line it fails on, what the parser expected, and (for a misspelled key) the
name you probably meant. A file with a mistake in it stops the program
instead of starting with settings that silently do not apply.

## Keeping things private

`/privacy` in either frontend lists what is sent where and what is
stored, from the settings in use. In short: chat goes to Twitch and
nowhere else; the emote providers (7TV, BetterTTV, FrankerFaceZ) are
asked which emotes a channel has, which tells them the channels you
read; the chat log and your login stay on this machine. Both of those
can be switched off:

```sh
liveside-tui --no-third-party-emotes   # contact only Twitch
liveside-tui --no-history              # keep no chat log on disk
```

[privacy.md](privacy.md) has the full list, including where the files
live and how to verify the claims yourself.

## The daemon

To keep chat running independently of the frontend, start `liveside-daemon` and
run `liveside-tui --daemon` or `liveside-gui --daemon`; several frontends
can attach to one daemon. To have it start with your session, install
`packaging/linux/liveside-daemon.service` as a systemd user service
(`~/.config/systemd/user/`, with `ExecStart` pointing at your binary if
it is not in `/usr/bin`) and run
`systemctl --user enable --now liveside-daemon.service`. A package
installs the service for you.

### Talking to it directly

```sh
cargo run -p liveside-daemon -- --demo &
socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/liveside/daemon.sock
{"type":"hello","protocol":1,"client":"socat"}
{"type":"request","id":1,"command":"login"}
{"type":"request","id":2,"command":"join","channel":"somechannel"}
```

Each line you type is one request; the daemon answers with JSON lines
(welcome, snapshot, then events and responses).


## Windows, through WSL2

Liveside runs on Windows 11 inside WSL2, whose WSLg shows its windows on
the Windows desktop. It was tried with Debian 13 inside: build it there as
from source (README, *Installing*), then see to these.

- **Links.** WSL has no browser of its own, so Liveside says it cannot
  open a link. Hand links to the Windows browser, once:

  ```sh
  mkdir -p ~/.local/share/applications
  cat > ~/.local/share/applications/windows-browser.desktop <<'END'
  [Desktop Entry]
  Type=Application
  Name=Windows browser
  Comment=Opens links in the default browser of Windows
  Exec=/mnt/c/Windows/explorer.exe %u
  MimeType=x-scheme-handler/http;x-scheme-handler/https;
  NoDisplay=true
  END
  gio mime x-scheme-handler/https windows-browser.desktop
  gio mime x-scheme-handler/http windows-browser.desktop
  ```

  It takes effect at once, without restarting Liveside. Until then,
  **Copy address** in the dialog works.
- **Sound** needs a player and the sounds, and then plays through WSLg:
  `sudo apt install pulseaudio-utils sound-theme-freedesktop`.
- **Notifications** do not reach Windows: WSLg has no notification
  service. Liveside says so once, and the chat goes on without them.
- **Drawing.** Debian's Mesa draws in software in WSL. Started as
  `GALLIUM_DRIVER=d3d12 liveside-gui`, the window draws on the GPU.
- **The terminal client** works in Windows Terminal, F-keys included, and
  sees by itself whether the colour scheme is light or dark.
- **The Flatpak** works too, with systemd running in WSL
  (`ps -p 1 -o comm=` says `systemd`). It opened links with the handler
  above set; without it, it was not tried.
