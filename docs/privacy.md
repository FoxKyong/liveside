# What this client sends, and what it keeps

Written 2026-09-23. The point of this page is that **you should not have to
read the source to know what the program does with your data**, so it lists
every outbound connection and every file, which feature causes it, and how
to turn it off.

The program can also answer this itself, from the settings it is actually
running with: type **`/privacy`** in the client. That list is built from the
same configuration that drives the behaviour, so it cannot drift from the
code the way a document can. If this page and `/privacy` ever disagree,
`/privacy` is the truth and this page is a bug.

## Leaves this machine

| Who | What they learn | When | Turn off |
|---|---|---|---|
| **Twitch** | Your account, the channels you join, the messages you send, your IP | Always, when not in `--demo` | Not possible; it is a Twitch client |
| **Twitch (IRC)** | The channels you join, from a second, anonymous connection; no account, no token | Only when the user list is set to `irc` | Settings: user list → `active` |
| **7TV** | Your IP, and which channels you join | On login and on every join, for emotes | `--no-third-party-emotes` |
| **BetterTTV** | The same | The same | `--no-third-party-emotes` |
| **FrankerFaceZ** | The same | The same | `--no-third-party-emotes` |
| **The sites a link leads through** | Your IP, and that this link was opened — by the client, not your browser | Only when you ask where a link goes, and only with the resolver set to `local` | Settings: Security → Where a link goes (off by default) |
| **VirusTotal** | A link you choose to check, your own API key, your IP. A link you send for a scan also becomes visible to VirusTotal's customers | Only when you ask about a link, and only with checking switched on | Settings: Security → VirusTotal (off by default) |

The IRC connection is there only to learn who is in a channel; it carries
no messages and does not log in, so Twitch sees it as an anonymous reader.
It exists only with `user_list = "irc"`, which is not the default.

Following a link is what an IP logger in chat is waiting for: a shortener
built to collect addresses gets yours as soon as the first request
arrives, opened in a browser or not. What following protects against is
the page itself, which is never rendered or even read; only the redirects
are. It never goes to a private address (`192.168.…`, `10.…`,
`localhost` and the like), whether or not anything is there: from this
machine it cannot tell which local networks your router reaches. And it
is never done by itself, only for the link you pick.

VirusTotal is never asked about a link by itself: only the one you pick,
from its menu or with `/link`. A first check only looks up a report that
already exists; a link VirusTotal does not know is sent to be scanned
only when you ask for that as a second step.

Streamlink, when you start it from the client, is a program of its own:
it talks to Twitch (and to the player) itself, and the client only
starts it with the channel's address. Liveside's login is never passed
to it; watching logged in takes a token from the browser in Streamlink's
own settings (docs/usage.md, "Watching with Streamlink").

That is the whole list. There is no telemetry, no crash reporting, no
update check, and no analytics. Nothing is sent to the author.

The terminal client never fetches emote pictures: it draws the codes as
text. The graphical client fetches each picture once, from the provider's
CDN (`static-cdn.jtvnw.net`, `cdn.7tv.app`, `cdn.betterttv.net`,
`cdn.frankerfacez.com`), and keeps it in the cache directory above. Those
requests carry your IP to the CDN, as any image on a web page would.

`/privacy` in the terminal client lists what the **engine** does; the
graphical client's own cache is not in that list yet.

A note on the emote providers: a chat client that shows 7TV/BTTV/FFZ emotes
must ask those services what the emotes are, and the request necessarily
carries your IP and the channel. `--no-third-party-emotes` avoids them
entirely; those emotes then show up as plain text, which is what Twitch
itself sends.

## Stays on this machine

| What | Where | Why | Turn off |
|---|---|---|---|
| Twitch access token | `$XDG_CONFIG_HOME/liveside/token.json`, mode 0600 in a 0700 directory | So you log in once | `/logout` asks Twitch to revoke it, then deletes it |
| VirusTotal API key | `$XDG_CONFIG_HOME/liveside/virustotal.key`, mode 0600 | Your own key, for checking links; not in `config.toml`, which people share | Delete the file, or switch checking off |
| Chat log | `$XDG_DATA_HOME/liveside/history/<channel>.jsonl`, mode 0600 in a 0700 directory | Restores a channel's recent lines when you join it again | `--no-history` |
| Open channels | `$XDG_STATE_HOME/liveside/channels.json`, mode 0600 | Opens again at the next start the channels that were open at the end | `reopen_channels = false` in `[chat]` |
| Window layout | `$XDG_STATE_HOME/liveside/layout.json` (graphical client only) | Puts docked channels and channels in windows of their own back where they were | Delete the file |
| Emote pictures | `$XDG_CACHE_HOME/liveside/emotes` (graphical client only) | So an emote is fetched once, not on every message | Delete the directory; `--no-third-party-emotes` stops all but Twitch's own |

The chat log is written only for channels you have joined, only while the
program runs, and only on this machine. It is plain JSON, one line per
message: you can read it, grep it, or delete any of it at any time, and the
program will simply have less to show next time. A channel's file is
rotated once it passes 16 MB, keeping one older generation, so a busy
channel costs at most about 32 MB.

**History is never fetched from anywhere.** Other clients load the messages
you missed from a third-party relay of Twitch chat; this one does not,
which is why a channel you join for the first time starts empty.

The demo backend (`--demo`) invents its chat locally, talks to nobody, and
is never written to the chat log. Its open channels and layout go to files
of their own (`channels-demo.json`, `layout-demo.json`), so trying it out
changes nothing for the real client.

## What it would take to check this

- Outbound connections: `ss -tp` while it runs, or run it under
  `strace -f -e trace=connect`.
- Files: everything is under `$XDG_CONFIG_HOME/liveside` and `$XDG_DATA_HOME/liveside`.
- In the source: every network call goes through `crates/twitch` (Twitch
  API in `helix.rs`, `auth.rs`, `eventsub.rs`; the emote providers in
  `emotes.rs`), and the only writer of user data is
  `crates/core/src/history.rs` plus the token file in `auth.rs`.
