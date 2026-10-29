# Writing a bot

A bot is a program of your own, in any language, that Liveside starts
and talks to. It never connects to Twitch and never sees a token:
Liveside is logged in already, reads the chat, and says what the bot
asks it to, as the logged-in account. Why it is built this way is in
`architecture.md`, *The user's own bots*.

A complete bot is in `examples/bots/weather.py`: `!pocasi [město]` or
`!weather [city]` answered from wttr.in. A package installs it to
`/usr/share/liveside/examples/bots/`. The Flatpak carries it in
`/app/share/liveside/examples/bots/`, which only the sandbox sees, while
a bot runs outside it (below), so copy it out first:

```sh
mkdir -p ~/bots
flatpak run --command=cat net.jifox.liveside.Liveside \
    /app/share/liveside/examples/bots/weather.py > ~/bots/weather.py
```

## Adding one

In the window: **Bots** above the chat, then **Add a bot**. Or by hand
in `config.toml`:

```toml
[[bots]]
name = "weather"
command = ["python3", "/home/me/bots/weather.py"]
# Where it runs: set it when the bot reads files beside it; the home
# directory if left out.
dir = "/home/me/bots"
# The channels it hears and may speak in, without `#`; every joined
# channel if left out.
channels = ["brightmaple"]
# Started with Liveside, rather than by pressing ▶.
autostart = true
# The most lines it may say in 30 seconds, 1 to 100. Twitch allows an
# account 20, or 100 in a channel it owns or moderates. 20 if left out.
rate = 20
# Seconds it has to finish once stopped before it is killed, 1 to 60;
# 2 if left out.
stop_after = 2
# Whether it hears what other bots say (never what it says itself); two
# bots that answer each other can go round in circles. Off if left out.
hear_bots = false
# Whether it hears the people the ignore rules hide. Off if left out.
hear_ignored = false
```

`command` is the program and its arguments as a list; nothing is run
through a shell. The name is how it is listed and must be unique.

### In the Flatpak

A bot runs outside the sandbox, on the system itself
(`flatpak-spawn --host`), so `command` and `dir` are paths there and the
program is the system's: `python3` is the one installed on the machine,
with its modules, not one inside the Flatpak. The settings file is the
Flatpak's own, `~/.var/app/net.jifox.liveside.Liveside/config/liveside/config.toml`,
apart from a native install's. A bot still ends with Liveside, as it
does natively. Why the Flatpak may do this is in `architecture.md`,
*Packages*.

## Talking

One JSON object per line, UTF-8, both ways. A line that is not
understood is ignored on both sides (Liveside writes it to the bot's
log), and fields not listed here may be added later, so a bot should
ignore what it does not know.

### What a bot is told (its standard input)

First, once:

```json
{"type":"hello","protocol":1,"name":"weather"}
```

Then a line per chat message in its channels:

```json
{"type":"message","channel":"brightmaple","id":"5f1c…","user":{"id":"123","login":"pineviewer","name":"Pineviewer"},"badges":["vip"],"text":"!pocasi Brno","action":false}
```

- `channel` is the channel's login, `id` the message's, to reply to.
- `user.name` is the display name, `user.login` the one to compare.
- `badges` are the badge sets the sender wears in that channel:
  `broadcaster`, `moderator`, `vip`, `subscriber` and the like, for a
  bot that answers only some people.
- `text` is the message as it was said, emotes as their names.
- `action` is true for `/me`.

A bot is told what you type yourself, but never what it said itself
(it would answer itself), nor messages from before it started. What
other bots say and what the ignore rules hide it is told only when
`hear_bots` and `hear_ignored` say so.

### What a bot can ask for (its standard output)

```json
{"type":"say","channel":"brightmaple","text":"Hello, chat"}
{"type":"reply","channel":"brightmaple","to":"5f1c…","text":"Brno: ⛅ +14°C"}
```

`say` is a plain message. `reply` answers the message whose `id` is in
`to`: Twitch shows it as a reply, starting with `@` and the name of whoever
asked (`@pineviewer Brno: …`), and your own question gets `@` and your own
name. The weather example replies; for answers without the name, write
`say` instead.

`channel` must be one of the bot's. At most `rate` lines in 30 seconds;
more are dropped and the log says so. Twitch limits a message to 500
characters.

Flush after every line (`print(..., flush=True)` in Python), or it may
sit in a buffer and never be sent.

### Its log

Whatever the bot writes to standard error is shown in its log in the
**Boti** window, so `print(..., file=sys.stderr)` is the way to debug.

## From the keyboard

`/bots` lists the bots and how each stands, in the channel it is typed
in; `/bot start <name>`, `/bot stop <name>` and `/bot log <name>` do
what they say. Both the window and the terminal client understand them.

## Stopping

When Liveside stops the bot, or closes, the bot's standard input ends:
finish then. A bot still running `stop_after` seconds later is killed. A bot
that ends by itself is shown as ended, with its exit code, and is not
started again until ▶ is pressed.

Only one copy of a bot runs on a machine: a second Liveside (a daemon
and a window, say) shows it as running elsewhere, and starts it itself
within five seconds of the first one closing or stopping it. Stopping it
in the second one ends the wait.
