# What is left to build

This is the list of everything Twitch offers that this client could
usefully do and does not do yet. The aim is a tool a broadcaster can keep
open instead of the dashboard, not only a moderator's chat window, so
"useful to whoever is streaming" counts as a reason to build something.

Two Twitch documents together are the whole of what is possible, and there
is no third: the **Helix API reference**
(<https://dev.twitch.tv/docs/api/reference>) for things a client can *do*,
and the **EventSub subscription types**
(<https://dev.twitch.tv/docs/eventsub/eventsub-subscription-types/>) for
things a client can be *told*. Both were read on **2026-09-25**; Twitch
adds to them, so anything dated later than that is not in this list.

`docs/chatty-features.md` is the other half of the picture: what Chatty
does that has nothing to do with Twitch's API (highlight rules, logs,
tabs, notifications). This file is only about what Twitch itself offers.

Each entry says what it needs — the endpoint or subscription, the OAuth
scope, and the screen it wants — because the work is usually the screen,
not the call.

## Where this stands today

Chat itself, and most of what happens around it. Besides the chat
subscriptions, the client hears follows, raids, shoutouts, the stream
going on and off, the title and category changing, Shield Mode, AutoMod's
queue and unban requests, every moderator action in the channel, polls,
predictions, channel point redemptions, Shared Chat, advertisement
breaks, and the money: subscriptions, gifts, bits, Hype Trains, goals and
charity donations.

Through Helix it can act: sending and whispering, the bans and timeouts,
delete, warn, clear, announce, the room modes and the chat delay,
moderators and VIPs, pinning, the channel's own information and its
schedule, shoutout, raid, marker, commercial, clips, Shield Mode, blocked
terms, suspicious users, polls, predictions, channel point rewards and
their queue. `docs/architecture.md` has the command list.

So: this client can act on a channel and see what happens in one. What
is left below is a short list of small things, and one feature waiting on
a beta API.

## 1. Moderation that needs a queue — done

The largest missing piece once, and the one moderators ask for first. All
four of these are the same shape of work: a list of things waiting for a
decision, a screen to show them in, and two buttons.

- ~~**The AutoMod queue**~~: done — `automod.message.hold` and
  `automod.message.update` (version 2) with **Manage Held AutoMod
  Messages**, scope `moderator:manage:automod`. Held messages appear in
  the chat with Allow and Deny beside them, and `/allow` and `/deny`
  answer the oldest from the keyboard. They also stand in the moderating
  panel, where they wait rather than scroll away.
- ~~**Our own messages being held**~~: done —
  `channel.chat.user_message_hold` and `.user_message_update`, scope
  `user:read:chat`. These turned out to be about the reader's own
  messages rather than other people's, so they are a notice in the
  channel, not a queue.
- ~~**Unban requests**~~: done — `channel.unban_request.create` and
  `.resolve`, with **Get Unban Requests** on joining and **Resolve Unban
  Requests** to answer; `/approve` and `/refuse`, the buttons in the line,
  or the moderating panel, where the whole plea is readable.
- ~~**Suspicious users**~~: done — `channel.suspicious_user.message`
  marks the message in the chat with why Twitch does not trust them and
  whether a moderator is watching or restricting them, and
  `.update` says in the channel when a moderator puts somebody under
  watch, restricts them or lets them go. There is nothing to mark in the
  second case, so it is a line, and it carries the `mod.` codes, which
  puts it in the moderation log beside the bans.

  `channel.moderate` does **not** carry these: its action list (read
  2026-10-11) has no monitor or restrict, so without this subscription
  one moderator's decision is invisible to the next.

  Both of them want `moderator:read:suspicious_users`, which this client
  did not ask for: `moderator:manage:suspicious_users` lets it decide
  about a user but does not stand in for the read scope. Twitch refused
  both subscriptions in silence, as it may for the rest of the extras, so
  the mark had never appeared in a real channel. The scope is asked for
  now, which means one more login for anyone holding an older token.

## 2. The moderation log — done

`channel.moderate` (version 2) reports every moderator action in a channel
— who banned whom, who changed a room mode, who deleted what — in one
subscription. It is what Chatty's moderation log shows, and it is also the
only way to see what *other* moderators are doing.

It wants the widest set of scopes of anything here (the `moderator:read:*`
family), because it carries every kind of action, and Twitch refuses the
subscription if one is missing. Done as notices in the channel and in the
moderating panel, where it can be read without the chat running through
it.

Related and small once the subscription exists: `channel.warning.send` and
`channel.warning.acknowledge` (scope `moderator:read:warnings`) — a warned
viewer acknowledging it is something the next moderator wants to know.

## 3. What a broadcaster needs to see

None of this is moderation; all of it is why someone would keep the window
open while streaming. These are all the same small job — subscribe, and
write a line into the chat the way `channel.chat.notification` lines are
already written — so they are cheap once the first one is done.

- ~~**Follows**~~: done — `channel.follow` (version 2), scope
  `moderator:read:followers`.
- ~~**Subscriptions**~~: done — `channel.subscribe`, `.message` (a resub
  with text) and `.gift`, scope `channel:read:subscriptions`. The
  per-person half of a gift is dropped on purpose; `.end` is not
  subscribed, since a subscription running out is not news in a chat.
- ~~**Cheers**~~: done — `channel.cheer`, scope `bits:read`.
  `channel.bits.use`, which would also cover Power-ups, is deliberately
  not subscribed: both would show a cheer twice.
- ~~**Raids arriving**~~: nothing to do — Twitch only fires `channel.raid`
  for raids that appear in chat, and those already reach us as chat
  notifications.
- ~~**Shoutouts**~~: done — `channel.shoutout.create` and `.receive`,
  scope `moderator:read:shoutouts`.
- ~~**Hype trains**~~: done — `channel.hype_train.begin` and `.end`
  (version 2) and **Get Hype Train Status** on joining, scope
  `channel:read:hype_train`. `.progress` is left out: a bar nobody reads
  in a chat window.
- ~~**Goals**~~: done — `channel.goal.begin` and `.end`, scope
  `channel:read:goals`. `.progress` is left out for the same reason.
- ~~**Charity campaigns**~~: done — `channel.charity_campaign.donate`,
  `.start` and `.stop`, scope `channel:read:charity`. `.progress` is left
  out, like the Hype Train's.
- ~~**Ad breaks**~~: done — `channel.ad_break.begin` says a break has
  started, `/ads` says when the next one is due (**Get Ad Schedule**) and
  `/snooze` pushes it back (**Snooze Next Ad**, scope
  `channel:manage:ads`, which is new).
- ~~**The stream itself**~~: done, but not through EventSub for every
  channel. `stream.online`, `stream.offline` and `channel.update` need no
  scope, and that is exactly what makes them cost: Twitch allows ten
  subscription points per token and charges one for every subscription
  about a user who has not authorized this client. Four channels spent
  the ten, after which Twitch refused *every* further subscription, chat
  included — which is why the fourth channel used to fail to join and why
  a live mark could stay green for a day. So these three are subscribed
  to only for our own channel, where they are free, and the channel list
  is kept right by asking **Get Streams** for every joined channel once a
  minute, in one call, correcting it where the answer differs. The same
  goes for `channel.shared_chat.*`, which costs for the same reason; a
  shared chat is noticed on joining instead.

  The other half of the same bug was an hourly round of new
  subscriptions, and it turned out to be this client's own doing. The
  renewal of the access token was capped at an hour although Twitch's
  token lives four, and a renewal rebuilt the EventSub socket — so every
  hour the client made all forty-odd subscriptions per channel again and
  left the previous set behind, enabled and still charged for. After a
  few hours the ten points were gone and Twitch refused everything, chat
  included. The cap is gone (renewal is now a quarter of an hour before
  the token ends), the renewal leaves a live socket alone, and whatever
  the last socket made is dropped before a new one subscribes.

  Twitch's own reconnect message is handled as well — the client follows
  the URL it carries, which keeps the subscriptions, and does not
  subscribe again on a session that came that way. That was not what was
  happening here, though: through a night of watching, the hourly
  reconnect was always ours.

  Verified with a six-hour run (2026-10-13, 00:08–06:09), Twitch's
  subscription list sampled every half hour: the cost stayed 0 of 10 and
  the count at 72 throughout. The token was renewed once, at 04:17, and
  the session it ran on carried on through the renewal with its
  subscriptions still enabled. Twitch did drop the connection twice
  (02:15 and 03:23, "connection reset by peer"); each time the client
  was back on the first attempt, said so on the status page, and left no
  subscriptions behind.
- ~~**Waiting is visible**~~: done — a channel appears in the list the
  moment it is asked for, with a turning wheel instead of the live mark
  (a braille spinner in the terminal, a `GtkSpinner` in the window), so
  the client never looks frozen while Twitch is answering. The row turns
  into the channel when it joins and is dropped, with the reason said,
  when it fails. The channels are joined side by side rather than one
  after another, and the dozen questions each join asks Twitch go out
  together: four channels used to take some twenty-five seconds and now
  take about six. Waiting is visible for everything else too: both
  frontends count what they have asked the engine and not been answered,
  and after a third of a second — long enough that an ordinary sent line
  never flashes it — turn a wheel beside the account, in the window a
  `GtkSpinner`, in the terminal the same braille frames.
- ~~**Shield Mode**~~: done — the events, **Get Shield Mode Status** on
  joining, and the state kept, so the menu has one item that says which
  way it goes instead of two that guess.
- ~~**Moderators and VIPs changing**~~: nothing to do —
  `channel.moderate` already reports them, so subscribing to
  `channel.moderator.*` and `channel.vip.*` would say it twice.
- ~~**Bans and unbans by anyone**~~: nothing to do — `channel.moderate`
  carries bans, unbans, timeouts and the lifting of them, which is what
  made another moderator's timeout visible in the first place.
- ~~**Shared Chat**~~: done — `channel.shared_chat.begin`, `.update` and
  `.end`, **Get Shared Chat Session** on joining, and both frontends mark
  a message with the channel it came from.
- ~~**Channel points**~~: done for
  `channel.channel_points_custom_reward_redemption.add` — a redemption is
  said in the channel with its cost and whatever was typed into it, scope
  `channel:read:redemptions`. `.update` and the automatic-reward variant
  wait for the queue, which is where they would mean something.

## 4. What a broadcaster needs to do

Each of these is a dialog of its own, so they are bigger than the events
above and worth doing one at a time, in the graphical client first.

- ~~**Polls**~~: done. **Create Poll**, **End Poll** and **Get Polls**
  (asked on joining one's own channel), with
  `channel.poll.begin/progress/end` for the running total: a bar in the
  window, `/poll` in the terminal. Scope `channel:manage:polls`, which is
  new, so this one needs logging in again.
- ~~**Predictions**~~: done. **Create Prediction**, **End Prediction**
  (lock, resolve, cancel) and **Get Predictions** (asked on joining), with
  `channel.prediction.*`: a bar in the window with a "pay out" button per
  outcome once betting closes, and /prediction, /lock, /payout in the
  terminal. Scope `channel:manage:predictions`, which is new.
- ~~**Channel point rewards**~~: done. Creating, changing, pausing and
  deleting them (**Moderate → Rewards…**, `/newreward`, `/pausereward`,
  `/rewardcost`), and the fulfil/refund queue in the moderation panel
  (`/queue`, `/fulfil`, `/refund`). Scope `channel:manage:redemptions`,
  which is new. Twitch allows answering and changing only to the
  application that created the reward, so both work for rewards made from
  here and nothing else; the rest are listed and marked as not ours.
- ~~**The rest of the channel's own settings**~~: done as a form
  (**Moderate → Channel…**, `/channel`): title, category, language, tags,
  stream delay and the content labels, all of **Modify Channel
  Information** under the scope this client already had. Branded content
  is the one field left out; Twitch sets it elsewhere.
- ~~**Clips**~~: done. **Create Clip** (`clips:edit`, new), answering with
  the edit URL. `/clip`, **F4** in both frontends, and an item in the
  Moderate menu.
- ~~**The stream schedule**~~: done. **Get Channel Stream Schedule**, the
  segment endpoints and vacation (`/schedule`, **Moderate → Schedule…**),
  scope `channel:manage:schedule`, which is new. Changing an existing
  broadcast's time is not offered: Twitch allows it only to partners and
  affiliates and only for non-recurring segments, so the window removes
  and adds instead.
- **Guest Star**: _put off until after the first release, and until the
  API moves on._ Twelve endpoints (the session, settings, invites and
  slots) and four events, all of them still marked BETA — and the events
  carry the version `beta` rather than a number, so they can change
  without a version to notice it by. Everything else this client
  subscribes to is version 1 or 2, where Twitch gives notice.

  It would want `channel:manage:guest_star` for one's own channel, and
  `moderator:manage:guest_star` as well to do any of it in a channel one
  merely moderates. That is another login for everybody, including the
  many who will never have a guest.

  When it is taken up, the reading half is worth doing first on its own:
  `channel:read:guest_star` and the four events say in the chat that a
  session has started, who took a slot and when it ended. Small, useful
  to a moderator, and it asks for nothing that can change somebody
  else's broadcast. Twitch has announced no end for Guest Star, so this
  is waiting on the beta, not on a rumour.

## 5. Lists worth showing

All of these are one call and a table, and all of them are built.

- ~~**Banned users**~~: done as `/banned`, with the end of each timeout.
- ~~**Moderators** and **VIPs**~~: done as `/mods` and `/vips`, said in a
  line in the terminal and opened as a list in the window.
- ~~**Blocked terms**~~: done — `/blockedterms` (also **Moderate →
  Blocked terms…**) opens a table in the window with when each term was
  added and until when it holds, a button to remove it and a field to add
  one; the list is asked for again after each change, so the table shows
  what Twitch did rather than a guess. The terminal says it in a line.
- ~~**Channels I moderate**~~: done — **Get Moderated Channels**, scope
  `user:read:moderated_channels`, which is new and so wants one more
  login. A button under the channel list opens them with a Join button
  each, the ones already joined marked; `/moderated` in the terminal.
- ~~**Channels I follow**~~: done — **Get Followed Channels** and **Get
  Followed Streams**, both scope `user:read:follows` (one more login
  again). A second button under the channel list opens a window with the
  live ones first, the most watched first, each with its category, title,
  viewers and how long it has run, then the rest by name; each has a
  Join button, and Refresh asks again. `/followed` in the terminal says
  the same in lines. The engine also asks every minute which of them
  stream (`FollowedStreams`): a channel that begins is said in the status
  view and, if switched on, notified; an open window keeps up.
- ~~**Followers** and **Subscribers**~~: done — **Get Channel Followers**
  (`moderator:read:followers`) and **Get Broadcaster Subscriptions**
  (`channel:read:subscriptions`), as `/followerlist` and `/sublist` and
  in the Moderate menu. Both lists can be long, so the newest 500 are
  read with the `total` Twitch sends (and the points, for subscribers).
  A channel one neither owns nor moderates gets its follower count and
  no names, which is Twitch's rule and is said as such.
- ~~**A user's card**~~: done — **Get Users** for the account's age,
  **Get Channel Followers** for since when they follow, **Get Banned
  Users** for whether they are already banned, **Get Broadcaster
  Subscriptions** for whether they subscribe and at what tier, plus their
  recent messages from our own buffer. The last of those works only in
  one's own channel, like the other two.

## 6. Whispers

~~**Send Whisper** and `user.whisper.message`~~: done, with the buffer it
needed — one thread per person in both frontends, `/w <name> <message>`
from anywhere, and what is typed in a thread whispered to that person.
Scopes `user:manage:whispers` and `user:read:whispers`, both new.

Twitch refuses a whisper from an account without a verified phone number
and says so only at that point; the refusal is passed on as it comes,
since it covers several reasons and guessing which would be worse than
Twitch's own words.

## 7. Not planned

- **Extensions, Drops, Conduits, Bits products, Analytics**: for developers
  and for extension authors, not for someone watching or running a chat.
- **Stream key** (**Get Stream Key**): a secret with no use in a chat
  client. Fetching it would mean asking for a scope that can take over a
  broadcast, for nothing.
- **Deleting videos**, **Update User** (the account's description),
  **Block / Unblock User** at the account level: they belong to the website
  rather than to a chat window. Blocking may be worth revisiting if people
  ask for it.
- **Redeeming channel point rewards as a viewer** (asked for beside the
  emote list, 2026-10-31): Twitch's API has no way to. It lets the
  broadcaster create, change and answer rewards, which Liveside does, but
  has no endpoint for a viewer to redeem one, nor to read how many points
  the viewer has. The website does it through Twitch's private interface,
  which is not for other programs.
- **Cheering Bits** (asked for at the same time): open. **Get Cheermotes**
  would give a list of cheermotes like the emote list, but Twitch
  documents no way for another program to spend Bits, and whether a
  `Cheer1` sent from Liveside spends one or arrives as plain text has not
  been tried (it costs a Bit). If it does spend it, the list is worth
  making; if not, there is nothing to make.
- **Opening Twitch's pop-out chat in the browser** for Bits and rewards:
  considered and dropped. It works only with a login in the default
  browser, which may be missing or another account than Liveside's, and
  would confuse more than it helps.

## The user's own bots

**Done** on 2026-10-19, designed from the owner's weather bot:
a bot is a program of its own that the engine starts and talks to in
JSON lines, so it needs no token or Twitch connection of its own. The
decisions are in `architecture.md` (*The user's own bots*), what a bot
has to do in `bots.md`, and the owner's bot, rewritten for it, in
`examples/bots/weather.py`.

1. **Done**: the protocol's commands and events, and `[[bots]]` in the
   settings.
2. **Done**: the engine runs bots: starts, stops, passes messages,
   limits what they say, keeps their logs, one copy per machine.
3. **Done**: a **Boti** window in the GUI: the list with ▶/⏹, state and
   log, and adding, changing and removing a bot; a crash is also said
   on the status page.
4. **Done**: `/bots` and `/bot start|stop|log <name>`, answered by the
   engine, so in both frontends; the terminal says a crash in its
   status buffer.

Later, if a bot needs them: a second account for bots to speak as, a
permission for one bot to moderate, and a way for one bot to be given
the token.

## Before the first public version

Agreed with the owner on 2026-10-19. The first public version is
**0.9.0**: complete, but not yet tried by many people; 1.0 follows once
it has been.

**Blocking publication**

1. ~~**Licence**~~: **GPL-3.0-or-later**, decided on 2026-10-27, so a
   fork stays open source too. Every dependency fits it (`architecture.md`,
   *Dependencies* and *Licensing*). Outside contributions are taken
   under the GPL alone, without a contributor agreement (decided the
   same day).
2. ~~**Application id.**~~ Decided on 2026-10-27:
   **`net.jifox.liveside.Liveside`**. The public repository goes to
   GitHub, but an id on a domain the owner keeps for good does not change
   when an account or a repository is renamed or moved, as `io.github.…`
   would; Flathub takes a domain id for an app hosted on GitHub. Its
   rules want the id's domain reachable over HTTPS, and `jifox.net` has
   no public website, so the id names `liveside.jifox.net` instead (the
   domain is every part but the last). The domain answers over HTTPS
   since 2026-10-28, and the page for it is `site/index.html`, with the
   screenshots and the icon copied beside it: no scripts and nothing from
   other sites, under a Content-Security-Policy that allows neither.
   **Not on Flathub for the first release**, decided on 2026-10-28:
   Liveside ships as a Flatpak bundle and an AUR package instead. Flathub
   is to be tried later, once the public repository has some history of
   its own; its rules want the AI's part disclosed, and the manifest and
   the submission written without one. The domain is verified then, with
   the token Flathub gives, as a TXT record at
   `_flathub.liveside.jifox.net`.
3. ~~**Twitch application.**~~ Whose Client ID the build carries, and whether
   the Developer Agreement allows it in a client anyone downloads. Read
   on 2026-10-27: Twitch calls Client IDs public and lets them be
   embedded, and the Device Code Flow has no secret to keep; but it
   wants one ID per application and may cut off one that others use, so
   a fork sets its own as `twitch.client_id` (done). Since 2026-10-27 the
   application is registered on a Twitch account kept for Liveside alone,
   not on the owner's streaming account, as a public client.
4. ~~**Clean the tree**~~ for the new public repository's single commit:
   done 2026-10-27. No secrets (the Client ID is public), no real
   channels or accounts in tests, docs and the demo (made-up names
   instead), no paths but `/home/me`-style examples. `CLAUDE.md` stays in
   the private repository: `.gitattributes` leaves it out of
   `git archive`, which the public commit is made from.

**Languages.** English and Czech, checked by the owner, and translations
into the languages whose script was checked on screen (2026-09-24):
Korean, Japanese, Russian, Thai and Greek, with Ukrainian beside Russian,
Spanish (one, neutral enough for Spain and Latin America, which is much
of Twitch's Spanish audience), Esperanto (chosen by hand: hardly any
system is set to it),
Chinese in both its written forms, Simplified (`zh-Hans`, mainland China,
Singapore) and Traditional (`zh-Hant`, Taiwan, Hong Kong; Twitch is
blocked in mainland China, so much of its Chinese-speaking audience
reads this one), Vietnamese, whose letters carry two marks at once (ệ,
ở) and was not among the scripts checked, so it is checked on screen in
both frontends first, and Arabic, whose right-to-left layout needs its
own check in both frontends. The catalogue has left `lang.rs` for a file
per language a translator can read, `crates/client/lang/<code>.toml`, and
a test finds lines a language lacks or placeholders it drops (done
2026-10-22). The English line once seen in the privacy window was not a
missing translation but the way to switch the IRC user list off, sent as
words ("settings: user list") where every other one is an option, a
command or a settings key; it is now `chat.user_list` (2026-10-23).

There is nobody to check the other languages yet, so they are machine
translations and say so: in the docs, and beside the language's name
where it is chosen. When volunteers have checked one, the note goes from
that language.

Done so far (2026-10-23): any number of languages, each listed by its
own name and marked when a machine translated it (`machine` and `note`
in its file); Spanish, the first machine translation, seen in both
frontends. It showed the header's buttons running past the window at
the default width, now cut short instead. The files are generated from
`en.toml`, so every comment and key stays in place and only the text
changes. Since then (2026-10-23) Russian, Ukrainian, Esperanto, Greek,
Korean, Japanese, Chinese in both forms (Traditional made from
Simplified by OpenCC with Taiwan's words), Thai and Vietnamese, each
seen in the window and the terminal. Korean showed the terminal
measuring text in letters rather than columns, and a wide letter
covering a box's border; both fixed. Longer words showed the join
button squeezing its field; it keeps room now. Arabic last, decided
with the owner: in both frontends, the window laid out from the right,
the terminal showing it as it can (right in Konsole, backwards in
Alacritty; `architecture.md`, *Language*). Every language on the list
is in, all machine translated. Left: volunteers to check them.

**Packages and platforms** (to be talked through before building):

- **Linux first.** GTK 4.10 is required, so Debian 12 (GTK 4.8) is out
  and Ubuntu 24.04, Debian 13 and Fedora are in; Rust 1.98 means building
  with rustup, not the distribution's compiler. **Flatpak** carries its
  own GTK, so it is the one package for every distribution; AUR beside
  it, `.deb` and `.rpm` if asked for. The same tests on each: login,
  chat, notifications, sounds, Streamlink, a bot. Built (2026-10-22,
  `packaging/flatpak/`) and tried on labwc in the demo: window,
  notifications, the bundled sound theme, a bot and Streamlink run on the
  host, a killed bot does not outlive the app (`architecture.md`,
  *Packages*). Tried on Plasma 6.8.1 (Arch) on 2026-10-23, installed from
  a single `.flatpak` bundle that fetched the GNOME runtime from Flathub
  by itself: listed in the launcher with its icon, dark and light
  followed live from Plasma's colour scheme (through the portal), Plasma's
  notification with the app's name and icon, the bundled sound, a bot and
  Streamlink on the host, a link opened in the browser through the
  portal. The look is Adwaita, with Plasma's own title bar. Tried on
  GNOME 50 (Ubuntu 26.04) on 2026-10-23, the same list: it showed in
  the overview only after logging in again, since the session that
  installs flatpak lacks its paths (Flathub's own instructions say to
  restart). GNOME found two faults Plasma had hidden, now fixed: a
  notification without an icon showed a blank symbol, and every
  notification that was to replace an earlier one was refused, because
  GNOME lets only the D-Bus connection that raised one replace it and
  notify-rust opened a new one each time. Notifications now go over one
  zbus connection of their own, which replaces them there too (tried on
  GNOME in the Flatpak, natively and from the terminal client, and
  natively on labwc, with the click and its activation token;
  `architecture.md`, *Notifications and sounds*). On Plasma 6.8.1 (the
  Flatpak, 2026-10-23) replacing worked, but a notification replaced
  after its bubble had gone was only updated in Plasma's list, never
  shown; it is now raised anew once the old one is off the screen, and
  the old one closed. Tried there: the popup each time, its icon, and a
  click that brings a minimized window back with the activation token.
  The same tried on GNOME 50: the second banner shows, and the list
  holds one notification, the old one closed.
  Built from source there too, with rustup (Ubuntu's Rust is 1.93) and
  `libgtk-4-dev` (GTK 4.22), in 5½ minutes on two cores; that build
  takes the system's Yaru look. Tried on Linux Mint 22.3 with
  Cinnamon 6.6 on 2026-10-26, the first X11 desktop, and the first with
  a GPU (VirGL in the VM, over the host's Radeon): until then GTK had
  only ever drawn in software. On X11 GTK chooses OpenGL over Vulkan by
  itself (`GskGLRenderer` in the Flatpak's GTK 4.24, `GskNglRenderer` in
  Mint's GTK 4.14). The Flatpak, a native build against GTK 4.14, the
  oldest GTK tried (three minutes on four cores; it takes Mint's own
  look), and the terminal client in GNOME Terminal: the notification
  with its icon, a click that brings a minimized window back and opens
  its channel (X11 needs no activation token), the sound, a link opened
  in Firefox through the portal, and Arabic in GNOME Terminal joined and
  right to left (VTE 0.76). Cinnamon found two faults, both fixed: a
  hidden page kept the chat at least 288 px wide, which GTK warned about
  as the window opened, and every notification played Cinnamon's sound
  beside Liveside's. Tried on Cosmic 1.10 (CachyOS, Wayland, VirGL) on
  2026-10-26: the Flatpak and a native build (Arch's Rust and GTK 4.24,
  six minutes on two cores). GTK on Wayland tries Vulkan, turns down
  the only device there is, the processor, and draws with OpenGL on the
  GPU. The notification, its click with an activation token bringing a
  minimized window back, the sound (Cosmic plays none of its own), a
  link opened in Firefox, the dark look followed. The notification's
  icon shows only from the next login after installing Flatpak, as on
  GNOME. Cosmic found one fault, fixed: it opens every window at two
  thirds of the screen, and the channel list shrank with the window and
  was cut off at the left edge. Arabic in Cosmic's terminal is joined
  and right to left, but the right border of the settings box is out
  of line, the terminal giving Arabic letters their own widths rather
  than the cells'. Tried on Debian 13 with Xfce 4.20 (X11, VirGL,
  GTK 4.18) on 2026-10-27, a native build: the notification in
  xfce4-notifyd 0.9.7 with its Open button, which opens its channel and
  brings the window to the front, minimized or behind another. The
  keyboard first stayed with the window that had it: the service sends
  no activation token, and GTK presented with an old time. Fixed: the
  window now takes the X server's time, and gets the keyboard, in its
  message line (also on labwc, with the token); a click on the bubble's text only closes it, as that service
  invokes no action for it. The sound played (paplay, into the VM's
  null sink), a link opened in Firefox ESR, the dark theme set through
  the old switch, as GTK 4.18 has no colour scheme, and the terminal
  client worked in xfce4-terminal. Under Weston nested in it, the
  people's panel with its fixed divider gave no width warning, with the
  window wider than the screen or maximized to 900 px. The metainfo
  (`packaging/linux/…metainfo.xml`, AppStream, for software
  centres) is in since 2026-10-28, valid by `appstreamcli`, with six
  of the screenshots in `docs/images/` (taken from the demo, linked at
  the `v0.9.0` tag on GitHub); on release its 0.9.0 entry gets the date,
  as `CHANGELOG.md`'s does. The terminal client on a light background
  had names too pale, as it lifted colours only for a dark one; since
  2026-10-28 it asks the terminal (OSC 11) or takes `ui.theme`, and
  corrects the other way there.
- **Testing.** One session drives every machine over SSH: screenshots
  taken on the machine and copied back, input through ydotool (xdotool
  on X11), so the context is not learnt again on each. Distributions are
  tried in short-lived VMs made for the test; the development VM is too
  small to hold them. The development VM draws in software and test VMs
  get a GPU through VirGL, so both ways GTK draws are seen.
- **Desktops.** GTK draws itself the same everywhere, so it works under
  Plasma; what differs is the look (Adwaita, not Breeze), whether
  following the desktop's dark mode works there, and the notification
  service. Tried on Plasma, GNOME, labwc (wlroots), Cinnamon (X11),
  Cosmic and Xfce (X11).
- **Windows.** Native builds need GTK from MSYS2 or gvsbuild, and the
  daemon's Unix socket and the D-Bus notifications have no direct
  counterpart; a build half as large was once made to run by a colleague
  with an AI's help, and every feature since adds to that. Proposed:
  Linux only, with WSL2 (WSLg shows Linux windows on Windows) as the
  documented way, tried by that colleague; a native build later, with a
  CI job that keeps it compiling. A native build also meets SmartScreen,
  which warns about an unsigned download, and Smart App Control, which
  blocks one outright: signing costs hundreds of dollars a year and
  even then the warning goes only as downloads build a reputation
  (Azure Trusted Signing is cheaper; whether it is open to an individual
  in Czechia is not checked). Inside WSL2 none of that applies, since
  what runs is a Linux program installed the Linux way. The price:
  `wsl --install` once, as administrator, with virtualization on;
  desktop notifications most likely do not reach Windows (to be tried;
  the chat goes on without them); sound works through WSLg; Flatpak
  needs systemd switched on in WSL. Tried by the colleague on
  2026-10-26 (Windows 11, WSL 2.7, Debian 13 in it, GTK 4.18, their AI
  following a prompt we sent, while they watched): it builds (1¾ minutes on 16
  cores) and all 364 tests pass; the window shows with its emotes,
  takes Czech letters, and copies both ways with Windows. Notifications
  do not reach Windows: WSLg has no notification service, and Liveside
  says so. Sound needs `pulseaudio-utils` and `sound-theme-freedesktop`
  from Debian, then plays through WSLg without restarting. Debian's Mesa
  draws in software there; `GALLIUM_DRIVER=d3d12` puts it on the GPU
  (seen on a Radeon RX 7800 XT). A link opened in the Windows browser
  once, though WSL had no handler for it; how is not known, and it was
  not tried again. The terminal client works in Windows Terminal, F-keys
  included. Two faults it found are fixed: `/join` typed on the status
  buffer of the terminal client did not open the channel, and a window
  attached to a daemon that stopped did not say so. GTK also warned that
  a box got 265 px of the 266 it needs: reproduced on a Debian 13 VM
  under Weston when the Review panel opens, and gone there once its
  divider lost its fixed position. A second round on 2026-10-28, on
  0.9.0 (`10a5a12`) the same way, confirmed the fixes: no width warning
  anywhere, `/join` from the status buffer, the window saying the daemon
  was gone, greying and joining again ~8 s after it came back, the
  terminal saying why it ended with status 1, and Windows Terminal
  answering the background query (One Half Light found light by
  itself). Logging in to Twitch worked, in the window and the terminal,
  and the login was kept. Links never opened without a handler (three
  tries; the one in the first round stays unexplained) and failed with
  GIO's bare "Operation not supported"; a `.desktop` file that hands
  them to `explorer.exe`, set with `gio mime`, opened them at once, so
  `usage.md` now has a section on WSL2, and the window says that no
  browser is set and where to read how. The address to log in at was
  plain text in the status page; it is a link now, through the same
  dialog. The Flatpak ran there as well, drawing with Vulkan on the
  CPU.
- **macOS.** GTK 4 builds through Homebrew; an app that opens without
  warnings needs signing and notarizing with a paid Apple developer
  account. Proposed: the colleague with a Mac tries a Homebrew build;
  nothing official until then. Tried by them on 2026-10-27 (macOS 27.2
  on Apple Silicon, GTK 4.24 and Rust from Homebrew, their AI following
  our prompt, the demo only): it builds without a warning (1⅓ minutes
  on 10 cores) and all 364 tests pass. The window has the native title
  bar, a sharp font, its name in the Dock, emotes, Czech letters typed
  with dead keys, the clipboard both ways, links opening in the default
  browser, and it follows the dark mode of macOS while running; GTK
  draws with GL. The terminal client works in Terminal.app, but F-keys
  need fn there, and Arabic falls apart (letters split by gaps, the
  frame of the settings broken), which Terminal.app's own shaping of
  Arabic most likely causes; VTE on Mint showed it right. Fixed from it:
  the demo's BibleThump, which Twitch retired, a demo daemon whose
  clients came up logged out, the terminal ending without a word in the
  shell when the daemon went, GTK 4.20's warning about the dark-theme
  switch, and the claim that the demo needs no network (its emote
  pictures come from Twitch). Made for the Mac afterwards, not yet tried
  on one: Cmd does what Ctrl does in the window's keys (they found Ctrl-F
  and Ctrl-R working and Cmd-F and Cmd-R not; Cmd-C and Cmd-V worked, as
  GTK's own), Cmd-Q quits (it did nothing; Ctrl-Q now quits on Linux
  too, as in the terminal), and sound plays through `afplay` from the
  Mac's own sounds in `/System/Library/Sounds` (there was no `pw-play`,
  `paplay` or sound theme, and the notice blamed only the theme). Not
  done: notifications (no D-Bus, and the notice shows its raw error);
  files in `~/.config` and `~/.cache` rather than `~/Library`. A window
  attached to a daemon that stops now says so (fixed after WSL, not in
  the build they had), greys its channels and connects again when the
  daemon is back, joining what it still shows (tried 2026-10-27 on labwc
  with the demo daemon, not on a Mac).
  Tried again by them on 2026-10-29 (same Mac, `10a5a12`, 371 tests
  passing), the second time with a Twitch login. Confirmed there: Cmd-F
  and Cmd-R as well as Ctrl, the sounds through `afplay` (Ping for a
  whisper, Hero for a stream going live), the window and the terminal
  attached to the demo daemon logged in, the window greying and coming
  back and the terminal ending with a word and status 1 when it went,
  logging in and out of Twitch with the token kept at 0600, and the
  terminal telling a light background from a dark one in Terminal.app
  and Ghostty. Cmd-Q still did nothing: GTK 4.24 reads `<Primary>` as
  Ctrl there, so the Mac now also gets `<Meta>q` (not yet tried on
  one). Copying, pasting and selecting all in a text field go by Cmd
  only, as GTK does on a Mac. Fixed from it: the hint of the message
  line telling someone logged in to log in, the restored lines' time
  fading into a grey dark background, the dark red of the ban-evader
  mark in Terminal.app's darker palette, the terminal's cursor left on
  the frame of the settings, and `--check-config` saying "from the 2.
  time". GTK still warns about skipped frames on the Mac, without
  anything to see.
- Release build, `.desktop` file and icon; bot examples installed to
  `/usr/share/liveside/examples` and the docs saying so; the daemon as a
  systemd user service. The `.desktop` file and a placeholder icon are in
  `packaging/linux/`, and the user service beside them
  (`liveside-daemon.service`, tried 2026-10-23: frontends attach, a
  channel stays joined between them, `systemctl --user stop` ends it
  cleanly and removes the socket). `docs/bots.md` says where the example
  is in a package and how to copy it out of the Flatpak. The package
  that installs them there is the AUR's `liveside`, whose PKGBUILD has a
  repository of its own, `foxkyong/aur-liveside`, pushed to the AUR as
  well (moved out of `packaging/arch/` on 2026-10-28, so that two copies
  cannot drift apart). It was first tried on 2026-10-26: built with
  makepkg from a tarball of the tree, 364 tests passing in its check(),
  clean in namcap but for the missing licence, url and maintainer, which
  waited for publication. Installed and tried:
  the window started from its `.desktop` file with its icon, the user
  service verified and started, its socket owner-only, the window
  attached to it. makepkg's LTO broke the link (aws-lc's C code as GCC
  bitcode, which Rust's lld cannot read), so it is off for this package.

**Also**

- ~~Versions from 0.9.0 with a changelog~~: `CHANGELOG.md`, from 0.9.0.
- Emotes fetched again after a network outage (put off on 2026-10-19;
  done 2026-10-23: a set a provider did not give is asked for again once
  the connection comes back after failed tries, not after Twitch merely
  moves the connection. Seen on a real outage on 2026-10-26, logged in,
  in three live channels, the daemon's network cut by a firewall on its
  own process: with 7TV, BTTV and FFZ unreachable the sets failed and
  their codes came as plain text; with the providers back but Twitch's
  connection never lost, nothing was fetched again, as meant; then 100
  seconds without any network (missed keepalive after 20 s, failed
  reconnects), and within a minute of it coming back the same codes came
  as 7TV emotes, 54 times plain text before and none failing after.)

**After it:** Guest Star (below), and for bots a second account, a
permission to moderate and an explicit way to be given the token.

## The order to build in

Sections 1 to 6 are built, and this is all that is left of Twitch's
side:

1. **Guest Star** (section 4): after the first release, and only once the
   API has left beta.

What is left beyond this is not Twitch's API but the client itself —
emoji, the rule language, notifications and the like — and
`docs/chatty-features.md` keeps that list.

Every step adds OAuth scopes, and a token granted before them is missing
them. The client already compares granted against wanted at login and says
what is missing, so each of these lands with one more line in that list
rather than with a puzzle for whoever is holding an old login.
