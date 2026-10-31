//! The chat view: one text buffer per channel, with tags for colour.
//!
//! A `GtkTextView` is used rather than a list of widgets, because it hands
//! the whole line to Pango: the shaping of Thai marks, Hangul blocks and
//! emoji is then the same as in every other GTK application, which is what
//! a chat client for the whole world needs.

use std::cell::Cell;
use std::rc::Rc;

use gtk::pango;
use gtk::prelude::*;
use liveside_client::chatters::{Chatter, Gathering};
use liveside_client::lang::{Strings, Verdict, fill, from_engine};
use liveside_proto::ids::Rgb;

use crate::theme::{Palette, readable};
use liveside_proto::model::{
    ChatMessage, Line, LineContent, LineFlags, LinkSource, LinkState, NoticeLevel,
};
use liveside_proto::{Login, MessageId, Timestamp};

use crate::emotes::Emotes;

/// One channel's messages.
pub struct ChatView {
    pub buffer: gtk::TextBuffer,
    /// The view the buffer is shown in, needed to place emote pictures.
    view: gtk::TextView,
    /// Where emote pictures come from; shared by every channel.
    emotes: Rc<Emotes>,
    /// Height of one line of text, so emotes match it.
    emote_height: i32,
    /// Where each message sits, so a click can find it again. The mark
    /// moves with the text, which trimming and emote pictures both do.
    messages: Vec<Shown>,
    /// Who the engine says is in the channel, for the user list. Empty
    /// unless membership tracking is on, and then the list falls back to
    /// whoever has talked.
    present: std::collections::BTreeSet<Login>,
    /// Lines kept; older ones are dropped so a long session stays light.
    lines: usize,
    limit: usize,
    /// The colours in force, which a change of theme replaces.
    palette: &'static Palette,
    /// What the view is put in, and what says there is more below.
    root: gtk::Overlay,
    scroll: gtk::ScrolledWindow,
    catch_up: gtk::Button,
    /// Whether the newest line is being followed. False while the user is
    /// reading further up, where moving the text under them would be
    /// rude — and where trimming has to wait, since dropping lines from
    /// the top would shift what they are reading.
    following: Rc<Cell<bool>>,
    /// Lines that arrived while they were reading back.
    unseen: Rc<Cell<usize>>,
    /// Whether links look like links. With links switched off they are
    /// plain text, and should not look clickable.
    links: bool,
    /// Whether each line starts with its time; not on a stream's screen.
    times: bool,
    /// The day of the newest line, to say so when the next one is from
    /// another.
    day: Option<liveside_client::days::Date>,
    /// What the window knows about links, shared by every view.
    reports: Rc<std::cell::RefCell<LinkReports>>,
    /// What a search found, oldest first, and which one it is at.
    found: Vec<(gtk::TextMark, gtk::TextMark)>,
    found_at: Option<usize>,
}

/// Answers about links, by address and by who answered.
pub type LinkReports = std::collections::HashMap<(String, LinkSource), LinkState>;

/// The order answers are written in after a link.
const SOURCES: [LinkSource; 2] = [LinkSource::VirusTotal, LinkSource::Redirects];

/// A message on screen: where it is, and who sent it. Kept only while
/// the message is in the buffer, so the user list follows the same
/// scrollback the terminal frontend shows.
struct Shown {
    mark: gtk::TextMark,
    /// Where the message itself starts, after the clock. A mark put in
    /// afterwards goes here, so it reads the same as one that arrived
    /// with the message.
    body: gtk::TextMark,
    id: MessageId,
    /// Who wrote it, by id, which is how Twitch names whoever it times
    /// out or bans.
    user: liveside_proto::UserId,
    login: Login,
    display_name: String,
    badges: Vec<liveside_proto::model::Badge>,
    /// The emote codes in the message, for Tab completion.
    emotes: Vec<String>,
}

impl ChatView {
    pub fn new(limit: usize, emotes: Rc<Emotes>, palette: &'static Palette) -> Self {
        let buffer = gtk::TextBuffer::new(None);
        // A tag's name is set when it is made; it cannot be changed later,
        // but its colour can, which is how a change of theme reaches the
        // lines already on screen.
        let table = buffer.tag_table();
        for name in [
            "time", "notice", "warning", "error", "event", "badge", "emote",
        ] {
            table.add(&make_tag(Some(name), None, false));
        }
        table.add(&make_tag(Some("name"), None, true));
        table.add(&gtk::TextTag::new(Some("link")));
        table.add(&gtk::TextTag::new(Some("picture")));
        table.add(&gtk::TextTag::new(Some("badge_picture")));
        for name in ["found", "found_here"] {
            table.add(&gtk::TextTag::new(Some(name)));
        }
        for name in [
            "vt_waiting",
            "vt_clean",
            "vt_dangerous",
            "vt_doubtful",
            "vt_unknown",
            "vt_info",
        ] {
            table.add(&gtk::TextTag::new(Some(name)));
        }
        table.add(&gtk::TextTag::new(Some("highlight")));
        table.add(&gtk::TextTag::new(Some("historical")));
        let deleted = gtk::TextTag::new(Some("deleted"));
        deleted.set_strikethrough(true);
        table.add(&deleted);
        // What a search found shows over everything, a highlighted line
        // included; the one it is at over the rest of what it found.
        for name in ["found", "found_here"] {
            if let Some(tag) = table.lookup(name) {
                tag.set_priority(table.size() - 1);
            }
        }
        paint(&table, palette);
        style_links(&table, palette, true);

        let view = gtk::TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .left_margin(6)
            .right_margin(6)
            .build();
        // Ctrl+C takes the same way as the menu's copy: emotes as their
        // codes and without the marks that only set directions apart.
        view.connect_copy_clipboard(|view| {
            view.stop_signal_emission_by_name("copy-clipboard");
            if let Some((start, end)) = view.buffer().selection_bounds() {
                view.clipboard().set_text(&text_between(&start, &end));
            }
        });
        let font = monospace_font();
        view.pango_context().set_font_description(&font);
        // An emote as tall as the text keeps the lines evenly spaced. GTK
        // stands a picture in the text on the baseline, so it would reach
        // above the letters by their descent and push the line apart;
        // lowered by that much it covers the line as the text does.
        let emote_height = line_height(&view, &font);
        let descent = view.pango_context().metrics(Some(&font), None).descent();
        if let Some(picture) = buffer.tag_table().lookup("picture") {
            picture.set_rise(-descent);
        }
        // A badge a little smaller than the line, as on Twitch, so badges
        // on lines one under another do not touch; still centred on it.
        if let Some(badge) = buffer.tag_table().lookup("badge_picture") {
            badge.set_rise(-(descent - BADGE_SHRINK * pango::SCALE / 2));
        }

        let scroll = gtk::ScrolledWindow::builder()
            .child(&view)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let catch_up = gtk::Button::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(6)
            .visible(false)
            .css_classes(["suggested-action"])
            .build();
        let root = gtk::Overlay::new();
        root.set_child(Some(&scroll));
        root.add_overlay(&catch_up);

        let following = Rc::new(Cell::new(true));
        let unseen = Rc::new(Cell::new(0usize));
        // Following is not a mode the user sets: it is simply whether they
        // are at the bottom, which is where the newest line is. Only a move
        // up leaves it, though: while GTK lays the text out the height
        // grows in steps, and a move down to the bottom could be measured
        // against a height that had grown again meanwhile, which read as
        // the reader having scrolled away.
        let last_value = Rc::new(Cell::new(0.0f64));
        scroll.vadjustment().connect_value_changed({
            let following = following.clone();
            let unseen = unseen.clone();
            let catch_up = catch_up.clone();
            move |adjustment| {
                let value = adjustment.value();
                let went_up = value < last_value.replace(value);
                let at_bottom = value >= adjustment.upper() - adjustment.page_size() - 1.0;
                if at_bottom {
                    following.set(true);
                    unseen.set(0);
                    catch_up.set_visible(false);
                } else if went_up {
                    following.set(false);
                }
            }
        });
        // The height of the text changes after the line that changed it:
        // once GTK has laid it out, and for a channel not on screen only
        // when it is shown, since a hidden view is not laid out at all. A
        // channel opened after its history came in stood somewhere in the
        // middle of it. A reader at the newest line stays there whenever
        // the height changes; one further up is left where they are.
        scroll.vadjustment().connect_changed({
            let following = following.clone();
            move |adjustment| {
                if following.get() {
                    adjustment.set_value(adjustment.upper() - adjustment.page_size());
                }
            }
        });
        catch_up.connect_clicked({
            let scroll = scroll.clone();
            move |_| to_bottom(&scroll)
        });
        // A view made narrower or wider (a channel docked beside it, the
        // window resized) wraps its lines again, and GTK moves the scroll
        // while it does. That is not the reader scrolling: a view that was
        // at the newest line goes back there once the text has settled.
        // The width is the horizontal adjustment's page.
        let width = Rc::new(Cell::new(0.0f64));
        scroll.hadjustment().connect_changed({
            let (following, unseen, catch_up) =
                (following.clone(), unseen.clone(), catch_up.clone());
            let scroll = scroll.clone();
            move |adjustment| {
                let was = width.replace(adjustment.page_size());
                if (was - adjustment.page_size()).abs() < 1.0 || !following.get() {
                    return;
                }
                let (following, unseen, catch_up) =
                    (following.clone(), unseen.clone(), catch_up.clone());
                let scroll = scroll.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                    following.set(true);
                    unseen.set(0);
                    catch_up.set_visible(false);
                    to_bottom(&scroll);
                });
            }
        });

        Self {
            buffer,
            view,
            emotes,
            emote_height,
            messages: Vec::new(),
            present: std::collections::BTreeSet::new(),
            lines: 0,
            limit,
            palette,
            root,
            scroll,
            catch_up,
            following,
            unseen,
            links: true,
            times: true,
            day: None,
            reports: Rc::default(),
            found: Vec::new(),
            found_at: None,
        }
    }

    /// Shares the window's answers about links, so a line that arrives
    /// with a link already checked says so at once.
    pub fn set_reports(&mut self, reports: Rc<std::cell::RefCell<LinkReports>>) {
        self.reports = reports;
    }

    /// Writes what is now known about `url` after every place it appears.
    pub fn show_report(&self, url: &str, strings: &'static Strings) {
        let table = self.buffer.tag_table();
        let mut links = Vec::new();
        table.foreach(|tag| {
            let found = unsafe { tag.data::<(String, String)>(LINK) }
                .is_some_and(|link| unsafe { link.as_ref() }.1 == url);
            if found {
                links.push(tag.clone());
            }
        });
        for tag in links {
            let Some(report) =
                unsafe { tag.data::<gtk::TextTag>(REPORT) }.map(|r| unsafe { r.as_ref() }.clone())
            else {
                continue;
            };
            // A line trimmed away takes the text but leaves the tags.
            if range_of(&self.buffer, &tag).is_none() {
                table.remove(&tag);
                table.remove(&report);
                continue;
            }
            self.write_report(&tag, &report, url, strings);
        }
    }

    /// Replaces what is written after one link.
    /// Every answer about the link, each in brackets of its own.
    fn write_report(
        &self,
        link: &gtk::TextTag,
        report: &gtk::TextTag,
        url: &str,
        strings: &'static Strings,
    ) {
        if let Some((mut start, mut end)) = range_of(&self.buffer, report) {
            self.buffer.delete(&mut start, &mut end);
        }
        let Some((_, mut at)) = range_of(&self.buffer, link) else {
            return;
        };
        let reports = self.reports.borrow();
        for source in SOURCES {
            let Some(state) = reports.get(&(url.to_owned(), source)) else {
                continue;
            };
            let (said, verdict) = liveside_client::lang::link_report(strings, source, state);
            let look = match verdict {
                Verdict::Waiting => "vt_waiting",
                Verdict::Clean => "vt_clean",
                Verdict::Dangerous => "vt_dangerous",
                Verdict::Doubtful => "vt_doubtful",
                Verdict::Unknown => "vt_unknown",
                Verdict::Info => "vt_info",
            };
            let offset = at.offset();
            self.buffer.insert(&mut at, &format!(" [{said}]"));
            let start = self.buffer.iter_at_offset(offset);
            self.buffer.apply_tag(report, &start, &at);
            self.buffer.apply_tag_by_name(look, &start, &at);
        }
    }

    /// Makes links look clickable, or like the rest of the text. Lines
    /// already on screen follow, since they share the one tag.
    pub fn set_links(&mut self, clickable: bool) {
        self.links = clickable;
        style_links(&self.buffer.tag_table(), self.palette, clickable);
    }

    /// The widget to put in a window: the text, and what tells the reader
    /// there is more below.
    pub fn root(&self) -> &gtk::Overlay {
        &self.root
    }

    /// Finds `query` in the text, ignoring case, marks every match, and
    /// goes to the newest one: in a chat that is usually the one wanted.
    /// Returns where it is and how many there are, newest counted first.
    pub fn search(&mut self, query: &str) -> Option<(usize, usize)> {
        self.end_search();
        if query.is_empty() {
            return None;
        }
        let flags = gtk::TextSearchFlags::CASE_INSENSITIVE | gtk::TextSearchFlags::TEXT_ONLY;
        let mut from = self.buffer.start_iter();
        while let Some((start, end)) = from.forward_search(query, flags, None) {
            self.buffer.apply_tag_by_name("found", &start, &end);
            self.found.push((
                self.buffer.create_mark(None, &start, true),
                self.buffer.create_mark(None, &end, false),
            ));
            from = end;
        }
        self.step_search(-1)
    }

    /// Goes to the next older match (`-1`) or newer one (`1`), wrapping
    /// round. Returns where it is, newest counted first, and how many.
    pub fn step_search(&mut self, direction: isize) -> Option<(usize, usize)> {
        // Lines dropped from the top took their matches with them; their
        // marks are left where the text was, with nothing between them.
        self.found.retain(|(start, end)| {
            self.buffer.iter_at_mark(start) != self.buffer.iter_at_mark(end)
        });
        let total = self.found.len();
        if total == 0 {
            self.found_at = None;
            return None;
        }
        if let Some((start, end)) = self.found_at.and_then(|at| self.found.get(at)) {
            let (start, end) = (
                self.buffer.iter_at_mark(start),
                self.buffer.iter_at_mark(end),
            );
            self.buffer.remove_tag_by_name("found_here", &start, &end);
        }
        let at = match self.found_at {
            None => total - 1,
            Some(at) => {
                (at.min(total - 1) as isize + direction).rem_euclid(total as isize) as usize
            }
        };
        self.found_at = Some(at);
        let (start, end) = &self.found[at];
        let (from, to) = (
            self.buffer.iter_at_mark(start),
            self.buffer.iter_at_mark(end),
        );
        self.buffer.apply_tag_by_name("found_here", &from, &to);
        // In the middle of the view, where the eye is.
        self.view.scroll_to_mark(start, 0.0, true, 0.0, 0.5);
        Some((total - at, total))
    }

    /// Takes the marks of a search away.
    pub fn end_search(&mut self) {
        let (start, end) = self.buffer.bounds();
        self.buffer.remove_tag_by_name("found", &start, &end);
        self.buffer.remove_tag_by_name("found_here", &start, &end);
        for (start, end) in self.found.drain(..) {
            self.buffer.delete_mark(&start);
            self.buffer.delete_mark(&end);
        }
        self.found_at = None;
    }

    /// The text itself, for the menu and for copying.
    /// Leaves the time off the lines that come from now on.
    pub fn set_times(&mut self, times: bool) {
        self.times = times;
    }

    pub fn view(&self) -> &gtk::TextView {
        &self.view
    }

    /// Back to the newest line, which starts following it again.
    pub fn jump_to_bottom(&self) {
        to_bottom(&self.scroll);
    }

    /// Whether the reader is somewhere further up.
    pub fn reading_back(&self) -> bool {
        !self.following.get()
    }

    /// Appends one line, in the language the window speaks. The newest
    /// line is followed only if the reader is already at it.
    pub fn push(&mut self, line: &Line, strings: &'static Strings) {
        if self.times {
            self.say_the_day(line, strings);
        }
        let buffer = &self.buffer;
        let mut end = buffer.end_iter();
        if self.lines > 0 {
            buffer.insert(&mut end, "\n");
        }
        let start_offset = buffer.end_iter().offset();
        if let LineContent::Message(message) = &line.content {
            let at = buffer.iter_at_offset(start_offset);
            let mark = buffer.create_mark(None, &at, true);
            let body = buffer.create_mark(None, &at, true);
            self.messages.push(Shown {
                mark,
                body,
                id: message.id.clone(),
                user: message.sender.id.clone(),
                login: message.sender.login.clone(),
                display_name: message.sender.display_name.clone(),
                badges: message.badges.clone(),
                emotes: message
                    .fragments
                    .iter()
                    .filter_map(|fragment| match fragment {
                        liveside_proto::model::Fragment::Emote { emote } => {
                            Some(emote.code.clone())
                        }
                        _ => None,
                    })
                    .collect(),
            });
        }

        if self.times {
            insert(buffer, &format!("{} ", clock(line.time)), &["time"]);
        }
        // The clock is in; from here on is the message itself.
        if let Some(shown) = self.messages.last() {
            let at = buffer.end_iter();
            buffer.move_mark(&shown.body, &at);
        }
        match &line.content {
            LineContent::Message(message) => self.message(message, &line.flags, strings),
            LineContent::UserNotice {
                system_text,
                message,
            } => {
                insert(buffer, &format!("* {system_text}"), &["event"]);
                if let Some(message) = message {
                    insert(buffer, " ", &[]);
                    self.fragments(message, strings);
                }
            }
            LineContent::Notice {
                level,
                text,
                code,
                args,
            } => {
                let tag = match level {
                    NoticeLevel::Info => "notice",
                    NoticeLevel::Warning => "warning",
                    NoticeLevel::Error => "error",
                };
                let said = from_engine(strings, code, args, text);
                insert(buffer, &format!("-- {said}"), &[tag]);
            }
        }

        // Flags colour the whole line, so they are applied last.
        if line.flags.deleted || line.flags.historical {
            let tag = if line.flags.deleted {
                "deleted"
            } else {
                "historical"
            };
            let start = buffer.iter_at_offset(start_offset);
            let end = buffer.end_iter();
            buffer.apply_tag_by_name(tag, &start, &end);
        }
        if line.flags.highlighted {
            self.highlight(start_offset, &line.flags);
        }

        self.lines += 1;
        if self.following.get() {
            self.trim();
            to_bottom(&self.scroll);
        } else {
            // Say how much is waiting, and leave the text where it is.
            self.unseen.set(self.unseen.get() + 1);
            self.catch_up.set_label(&fill(
                strings.catch_up,
                &[("count", &self.unseen.get().to_string())],
            ));
            self.catch_up.set_visible(true);
        }
    }

    fn message(&self, message: &ChatMessage, flags: &LineFlags, strings: &'static Strings) {
        let buffer = &self.buffer;
        // Why Twitch does not trust them, beside what they said: a list
        // elsewhere would be read long after this scrolled past.
        if let Some(suspicion) = &flags.suspicion {
            insert(
                buffer,
                &format!(
                    "[{}] ",
                    liveside_client::lang::suspicion(strings, suspicion)
                ),
                &["error"],
            );
        }
        // A Shared Chat session puts other channels' messages in here;
        // which channel one came from is what tells a conversation from
        // a crowd.
        if let Some(source) = &message.shared_source {
            insert(buffer, &format!("#{} ", source.channel.login), &["time"]);
        }
        self.badges(&message.badges);
        let colour = readable(
            message.color.unwrap_or_else(|| default_colour(message)),
            self.palette.dark,
        );
        let name_tag = gtk::TextTag::new(None);
        name_tag.set_foreground(Some(&format!(
            "#{:02x}{:02x}{:02x}",
            colour.r, colour.g, colour.b
        )));
        name_tag.set_weight(700);
        buffer.tag_table().add(&name_tag);
        // The name and what they said are each set apart, so the way one
        // runs cannot turn the rest of the line round: an English message
        // in a line that starts in Arabic keeps its question mark at its
        // end, and a direction mark hidden in a message stops with it.
        insert(buffer, ISOLATE, &[]);
        let mut end = buffer.end_iter();
        let offset = end.offset();
        buffer.insert(&mut end, &message.sender.display_name);
        let start = buffer.iter_at_offset(offset);
        let end = buffer.end_iter();
        buffer.apply_tag(&name_tag, &start, &end);
        insert(buffer, END_ISOLATE, &[]);
        // A /me reads as what they do: no colon, and in their colour.
        let action = message.kind == liveside_proto::model::MessageKind::Action;
        insert(buffer, if action { " " } else { ": " }, &[]);

        insert(buffer, ISOLATE, &[]);
        let said_from = buffer.end_iter().offset();
        self.fragments(message, strings);
        insert(buffer, END_ISOLATE, &[]);
        if action {
            let said_tag = gtk::TextTag::new(None);
            said_tag.set_foreground(Some(&format!(
                "#{:02x}{:02x}{:02x}",
                colour.r, colour.g, colour.b
            )));
            buffer.tag_table().add(&said_tag);
            buffer.apply_tag(
                &said_tag,
                &buffer.iter_at_offset(said_from),
                &buffer.end_iter(),
            );
        }
        // How many times they have now said it, after what they said.
        if flags.repeated > 0 {
            insert(buffer, &format!(" (×{})", flags.repeated), &["time"]);
        }
    }

    /// Makes the text between two offsets a link to `url`. A tag of its
    /// own carries the address, so a click can find where the link goes;
    /// the shared tag gives the look. The second tag returned, empty for
    /// now, marks where what is known about the link is written after it.
    fn make_link(&self, from: i32, to: i32, text: &str, url: &str) -> (gtk::TextTag, gtk::TextTag) {
        let tag = gtk::TextTag::new(None);
        let report = gtk::TextTag::new(None);
        unsafe {
            tag.set_data(LINK, (text.to_owned(), url.to_owned()));
            tag.set_data(REPORT, report.clone());
        }
        self.buffer.tag_table().add(&tag);
        self.buffer.tag_table().add(&report);
        let start = self.buffer.iter_at_offset(from);
        let end = self.buffer.iter_at_offset(to);
        self.buffer.apply_tag(&tag, &start, &end);
        self.buffer.apply_tag_by_name("link", &start, &end);
        (tag, report)
    }

    /// Makes `url` a link where the last line says it, as in a line of
    /// chat. For a notice of Liveside's own that names an address, such
    /// as the one to log in at.
    pub fn link_in_last_line(&self, url: &str) {
        let end = self.buffer.end_iter();
        let mut line_start = end;
        line_start.set_line_offset(0);
        if let Some((start, found_end)) =
            end.backward_search(url, gtk::TextSearchFlags::TEXT_ONLY, Some(&line_start))
        {
            self.make_link(start.offset(), found_end.offset(), url, url);
        }
    }

    fn fragments(&self, message: &ChatMessage, strings: &'static Strings) {
        use liveside_proto::model::Fragment;
        for fragment in &message.fragments {
            match fragment {
                Fragment::Emote { emote } => self.emote(emote),
                Fragment::Mention { text, .. } => insert(&self.buffer, text, &["name"]),
                Fragment::Link { text, url } => {
                    let mut end = self.buffer.end_iter();
                    let offset = end.offset();
                    self.buffer.insert(&mut end, text);
                    let to = self.buffer.end_iter().offset();
                    let (tag, report) = self.make_link(offset, to, text, url);
                    let known = SOURCES
                        .iter()
                        .any(|source| self.reports.borrow().contains_key(&(url.clone(), *source)));
                    if known {
                        self.write_report(&tag, &report, url, strings);
                    }
                }
                other => insert(&self.buffer, other.text(), &[]),
            }
        }
    }

    /// Badges are written as the letters the terminal uses and become
    /// Twitch's own pictures once those arrive — a badge whose picture
    /// cannot be had stays as a letter rather than as a gap. A badge the
    /// backend could not look up has no picture to wait for and simply
    /// keeps its letter.
    fn badges(&self, badges: &[liveside_proto::model::Badge]) {
        if badges.is_empty() {
            return;
        }
        for (i, badge) in badges.iter().enumerate() {
            // Apart from the next badge; the last is followed by a space.
            let gap = if i + 1 < badges.len() { BADGE_GAP } else { 0 };
            let symbol = badge_symbol(badge);
            if badge.image.is_empty() {
                if let Some(symbol) = symbol {
                    insert(&self.buffer, &symbol.to_string(), &["badge"]);
                }
                continue;
            }
            // Something has to stand there meanwhile, or the name would
            // jump sideways when the picture lands.
            let placeholder = symbol.unwrap_or('·').to_string();
            let tooltip = if badge.title.is_empty() {
                badge.set_id.clone()
            } else {
                badge.title.clone()
            };
            self.replace_with_picture(
                &placeholder,
                "badge",
                badge_key(badge),
                badge.image.clone(),
                tooltip,
                gap,
            );
        }
        insert(&self.buffer, " ", &[]);
    }

    /// Writes `text`, then replaces it with the picture at `url` once that
    /// has been fetched. The end mark keeps left gravity, so what is
    /// written after the text while the picture is on its way stays to its
    /// right. The start mark has right gravity: when the picture just
    /// before lands (two badges, two emotes side by side), it goes in at
    /// this start and must stay to its left, or this replacement would
    /// take it away again along with the text.
    fn replace_with_picture(
        &self,
        text: &str,
        tag: &str,
        key: String,
        url: String,
        tooltip: String,
        gap: i32,
    ) {
        let buffer = &self.buffer;
        let start_offset = buffer.end_iter().offset();
        insert(buffer, text, &[tag]);
        let start = buffer.create_mark(None, &buffer.iter_at_offset(start_offset), false);
        let end = buffer.create_mark(None, &buffer.end_iter(), true);

        let buffer = buffer.clone();
        let view = self.view.clone();
        let emotes = self.emotes.clone();
        let (height, lowered) = if tag == "badge" {
            (self.emote_height - BADGE_SHRINK, "badge_picture")
        } else {
            (self.emote_height, "picture")
        };
        self.emotes.request_url(key, url, move |animation| {
            if start.is_deleted() || end.is_deleted() {
                return;
            }
            let mut from = buffer.iter_at_mark(&start);
            let mut to = buffer.iter_at_mark(&end);
            // The line may have been trimmed off the top by now, which
            // leaves both marks together where it was.
            if from.offset() < to.offset() {
                buffer.delete(&mut from, &mut to);
                let anchor = buffer.create_child_anchor(&mut from);
                let mut before = from;
                before.backward_char();
                buffer.apply_tag_by_name(lowered, &before, &from);
                let picture = emotes.picture(animation, &tooltip, height);
                picture.set_margin_end(gap);
                view.add_child_at_anchor(&picture, &anchor);
            }
            buffer.delete_mark(&start);
            buffer.delete_mark(&end);
        });
    }

    /// An emote is written as its code and becomes a picture once the
    /// picture has been fetched. That order matters: an emote whose picture
    /// cannot be had stays readable instead of leaving a hole.
    fn emote(&self, emote: &liveside_proto::model::EmoteRef) {
        let Some(url) = emote.image_url(1) else {
            insert(&self.buffer, &emote.code, &["emote"]);
            return;
        };
        let key = format!("{:?}-{}", emote.provider, emote.id);
        self.replace_with_picture(&emote.code, "emote", key, url, emote.code.clone(), 0);
    }

    /// Writes a message AutoMod is holding, with the two decisions as
    /// buttons in the line itself: a queue of one is not worth a window,
    /// and the text to judge is right there beside them.
    pub fn push_held(
        &mut self,
        held: &liveside_proto::event::HeldMessage,
        strings: &'static Strings,
        decide: impl Fn(bool) + 'static,
    ) {
        let said = fill(
            strings.held_message,
            &[
                ("user", &held.sender.display_name),
                ("detail", &held.detail),
                ("text", &held.text),
            ],
        );
        self.push_decision(said, strings, decide);
    }

    /// Something waiting for a yes or a no, with both beside it.
    pub fn push_decision(
        &mut self,
        said: String,
        strings: &'static Strings,
        decide: impl Fn(bool) + 'static,
    ) {
        let line = Line {
            time: Timestamp::now(),
            content: LineContent::Notice {
                level: NoticeLevel::Warning,
                text: said,
                code: String::new(),
                args: Vec::new(),
            },
            flags: LineFlags::default(),
        };
        self.push(&line, strings);

        let decide = Rc::new(decide);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (label, allow) in [(strings.held_allow, true), (strings.held_deny, false)] {
            let button = gtk::Button::builder()
                .label(label)
                .css_classes(["flat"])
                .build();
            let decide = decide.clone();
            let buttons_for_click = buttons.clone();
            button.connect_clicked(move |_| {
                // Whatever was decided, the choice is over; leaving the
                // buttons would invite a second click on a message that
                // is already gone.
                buttons_for_click.set_sensitive(false);
                decide(allow);
            });
            buttons.append(&button);
        }
        // On a line of their own: after the text, they sat beside it in a
        // wide view and went under it only where it wrapped. A line, so
        // trimming counts it with the rest.
        let mut at = self.buffer.end_iter();
        self.buffer.insert(&mut at, "\n");
        self.lines += 1;
        let anchor = self.buffer.create_child_anchor(&mut at);
        self.view.add_child_at_anchor(&buttons, &anchor);
    }

    /// Marks a line just written as highlighted: in the theme's colour,
    /// or in the ones its rule asked for, over that.
    fn highlight(&self, start_offset: i32, flags: &LineFlags) {
        let buffer = &self.buffer;
        let start = buffer.iter_at_offset(start_offset);
        let end = buffer.end_iter();
        buffer.apply_tag_by_name("highlight", &start, &end);
        if flags.highlight_color.is_none() && flags.highlight_background.is_none() {
            return;
        }
        let hex = |rgb: Option<liveside_proto::Rgb>| rgb.map(|c| c.to_string()).unwrap_or_default();
        let name = format!(
            "rule:{}:{}",
            hex(flags.highlight_color),
            hex(flags.highlight_background)
        );
        let table = buffer.tag_table();
        if table.lookup(&name).is_none() {
            let tag = gtk::TextTag::new(Some(&name));
            if let Some(color) = flags.highlight_color {
                tag.set_foreground(Some(&color.to_string()));
            }
            if let Some(background) = flags.highlight_background {
                tag.set_background(Some(&background.to_string()));
            }
            table.add(&tag);
            // Over the theme's highlight, and under what a search found,
            // which has to show over everything.
            for name in ["found", "found_here"] {
                if let Some(found) = table.lookup(name) {
                    found.set_priority(table.size() - 1);
                }
            }
        }
        buffer.apply_tag_by_name(&name, &start, &end);
    }

    /// The message shown at `iter`, for the menu a click opens.
    pub fn message_at(&self, iter: &gtk::TextIter) -> Option<(MessageId, Login)> {
        let line = iter.line();
        // The last message that starts at or above the clicked line.
        self.messages
            .iter()
            .rfind(|shown| {
                !shown.mark.is_deleted() && self.buffer.iter_at_mark(&shown.mark).line() <= line
            })
            .map(|shown| (shown.id.clone(), shown.login.clone()))
    }

    /// Repaints for another theme. The lines already written change with
    /// it, since they carry tags rather than colours — except the colour a
    /// viewer chose for their own name, which was worked out when the line
    /// arrived and stays as it was.
    pub fn set_palette(&mut self, palette: &'static Palette) {
        self.palette = palette;
        paint(&self.buffer.tag_table(), palette);
        style_links(&self.buffer.tag_table(), palette, self.links);
    }

    /// What one person has said lately, newest last, as the buffer still
    /// holds it. For their card, where their own words say more than any
    /// number Twitch can give.
    pub fn messages_from(&self, login: &Login, most: usize) -> Vec<String> {
        let mut found: Vec<String> = self
            .messages
            .iter()
            .rev()
            .filter(|shown| &shown.login == login && !shown.mark.is_deleted())
            // One message is one line; what follows it in the buffer
            // belongs to whatever came next, which may well be a notice
            // about somebody else.
            .filter_map(|shown| {
                self.message_text(&shown.id)
                    .map(|text| text.lines().next().unwrap_or_default().to_owned())
            })
            .filter(|text| !text.is_empty())
            .take(most)
            .collect();
        found.reverse();
        found
    }

    /// The whole line a message occupies, as it is shown, for copying.
    /// Emote pictures come back as their codes.
    /// Marks a message already on screen as coming from somebody Twitch
    /// does not trust. The reason goes in front of it, where the message
    /// is, rather than in a line of its own further down.
    pub fn mark_suspicious(
        &mut self,
        id: &MessageId,
        suspicion: liveside_proto::model::Suspicion,
        strings: &'static Strings,
    ) {
        let Some(at) = self
            .messages
            .iter()
            .position(|shown| &shown.id == id && !shown.mark.is_deleted())
        else {
            return;
        };
        let said = format!(
            "[{}] ",
            liveside_client::lang::suspicion(strings, &suspicion)
        );
        let mut start = self.buffer.iter_at_mark(&self.messages[at].body);
        let offset = start.offset();
        self.buffer.insert(&mut start, &said);
        let start = self.buffer.iter_at_offset(offset);
        let end = self
            .buffer
            .iter_at_offset(offset + said.chars().count() as i32);
        self.buffer.apply_tag_by_name("error", &start, &end);
    }

    pub fn message_text(&self, id: &MessageId) -> Option<String> {
        let at = self
            .messages
            .iter()
            .position(|shown| &shown.id == id && !shown.mark.is_deleted())?;
        let start = self.buffer.iter_at_mark(&self.messages[at].mark);
        let end = self
            .messages
            .iter()
            .skip(at + 1)
            .find(|shown| !shown.mark.is_deleted())
            .map(|shown| self.buffer.iter_at_mark(&shown.mark))
            .unwrap_or_else(|| self.buffer.end_iter());
        Some(text_between(&start, &end).trim_end().to_owned())
    }

    /// Replaces who the engine says is in the channel.
    pub fn set_present(&mut self, logins: Vec<Login>) {
        self.present = logins.into_iter().collect();
    }

    /// Notes that someone joined or left, as Twitch IRC reports it.
    pub fn set_here(&mut self, login: Login, here: bool) {
        if here {
            self.present.insert(login);
        } else {
            self.present.remove(&login);
        }
    }

    /// The message to answer with Ctrl-R: the newest one, or the one
    /// before `after` when a reply is already being written.
    pub fn reply_target(&self, after: Option<&MessageId>) -> Option<(MessageId, Login)> {
        let mut shown = self
            .messages
            .iter()
            .rev()
            .filter(|shown| !shown.mark.is_deleted());
        if let Some(after) = after {
            // Past the one being answered; if it has scrolled away, start
            // again from the newest rather than find nothing.
            let mut rest = shown.clone();
            if rest.any(|shown| &shown.id == after) {
                shown = rest;
            }
        }
        shown
            .next()
            .map(|shown| (shown.id.clone(), shown.login.clone()))
    }

    /// Names and emote codes for completing `prefix`, from the messages
    /// still on screen, newest speaker first.
    pub fn completions(&self, prefix: &str) -> Vec<String> {
        liveside_client::input::names_and_emotes(
            prefix,
            self.messages
                .iter()
                .rev()
                .filter(|shown| !shown.mark.is_deleted())
                .map(|shown| {
                    (
                        shown.display_name.as_str(),
                        shown.emotes.iter().map(String::as_str).collect(),
                    )
                }),
        )
    }

    /// The people to show in the user list, ranked then alphabetical.
    pub fn chatters(&self) -> Vec<Chatter> {
        let mut gathering = Gathering::default();
        for shown in self.messages.iter().rev() {
            if shown.mark.is_deleted() {
                continue;
            }
            gathering.talked(&shown.login, &shown.display_name, &shown.badges);
        }
        for login in &self.present {
            gathering.present(login);
        }
        gathering.finish()
    }

    /// Strikes a deleted message through, where it is: what was removed
    /// stays readable for a moderator, and plainly removed.
    /// A line with the date before the first line of a day; it counts
    /// as a line, so it goes when the lines around it are trimmed.
    fn say_the_day(&mut self, line: &Line, strings: &'static Strings) {
        use liveside_client::days;
        let Some(day) = days::day_of(line.time) else {
            return;
        };
        let previous = self.day.replace(day);
        let Some(day) = days::day_starts(previous, day, days::today()) else {
            return;
        };
        let mut end = self.buffer.end_iter();
        if self.lines > 0 {
            self.buffer.insert(&mut end, "\n");
        }
        insert(
            &self.buffer,
            &format!("── {} ──", days::day_line(strings, day)),
            &["time"],
        );
        self.lines += 1;
    }

    pub fn delete_message(&mut self, id: &MessageId) {
        self.strike(|shown| &shown.id == id);
    }

    /// Strikes through everything someone said, as a timeout or a ban
    /// removes it.
    pub fn delete_user(&mut self, user: &liveside_proto::UserId) {
        self.strike(|shown| &shown.user == user);
    }

    fn strike(&mut self, which: impl Fn(&Shown) -> bool) {
        for shown in self
            .messages
            .iter()
            .filter(|s| !s.mark.is_deleted() && which(s))
        {
            let start = self.buffer.iter_at_mark(&shown.mark);
            let mut end = start;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            self.buffer.apply_tag_by_name("deleted", &start, &end);
        }
    }

    /// Takes every line away, for a view that said there was nothing yet
    /// and now has something.
    pub fn clear(&mut self) {
        self.buffer.set_text("");
        self.lines = 0;
        self.day = None;
        self.messages.clear();
        self.found.clear();
        self.found_at = None;
        // Nothing is left to be behind on.
        self.following.set(true);
        self.unseen.set(0);
        self.catch_up.set_visible(false);
    }

    /// Drops the oldest lines once the buffer grows past its limit.
    fn trim(&mut self) {
        // Dropping lines from the top moves everything below them, which
        // would pull the text out from under a reader who is up there.
        if self.lines <= self.limit || !self.following.get() {
            return;
        }
        let drop = self.lines - self.limit;
        let mut start = self.buffer.start_iter();
        let mut end = self.buffer.iter_at_line(drop as i32).unwrap_or(start);
        self.buffer.delete(&mut start, &mut end);
        self.lines = self.limit;
        // Marks inside the deleted text are gone; forget them.
        self.messages.retain(|shown| !shown.mark.is_deleted());
    }
}

/// Puts the newest line back in view.
fn to_bottom(scroll: &gtk::ScrolledWindow) {
    let adjustment = scroll.vadjustment();
    // After a change the height is settled only once GTK has laid the
    // text out again, so this waits for that.
    glib::idle_add_local_once(move || {
        adjustment.set_value(adjustment.upper() - adjustment.page_size());
    });
}

/// The text between two places, with each emote picture written as its
/// code. Taking the buffer's text plainly would give the placeholder
/// character that stands for a widget, and the emote would be lost.
pub fn text_between(start: &gtk::TextIter, end: &gtk::TextIter) -> String {
    let mut out = String::new();
    let mut at = *start;
    while at < *end {
        match at.child_anchor() {
            // Only there for the layout; nobody pasting wants them.
            None if matches!(at.char(), '\u{2068}' | '\u{2069}') => {}
            Some(anchor) => {
                let code = anchor
                    .widgets()
                    .first()
                    .and_then(|widget| widget.tooltip_text())
                    .map(|code| code.to_string())
                    .unwrap_or_default();
                out.push_str(&code);
            }
            None => out.push(at.char()),
        }
        if !at.forward_char() {
            break;
        }
    }
    out
}

/// Gives each tag the colour this theme asks for.
fn paint(table: &gtk::TextTagTable, palette: &Palette) {
    let set = |name: &str, colour: &str| {
        if let Some(tag) = table.lookup(name) {
            tag.set_foreground(Some(colour));
        }
    };
    set("time", palette.time);
    set("notice", palette.notice);
    set("warning", palette.warning);
    set("error", palette.error);
    set("event", palette.event);
    set("badge", palette.badge);
    set("emote", palette.emote);
    set("historical", palette.faded);
    set("deleted", palette.faded);
    if let Some(tag) = table.lookup("highlight") {
        tag.set_background(Some(palette.highlight));
    }
    if let Some(tag) = table.lookup("found") {
        tag.set_background(Some(palette.found));
    }
    if let Some(tag) = table.lookup("found_here") {
        tag.set_background(Some(palette.found_here));
    }
    set("vt_waiting", palette.faded);
    set("vt_clean", palette.badge);
    set("vt_unknown", palette.warning);
    set("vt_doubtful", palette.warning);
    set("vt_info", palette.event);
    set("vt_dangerous", palette.error);
    if let Some(tag) = table.lookup("vt_dangerous") {
        tag.set_weight(700);
    }
}

/// Where a link's tag keeps the link: the text as written, and the address.
const LINK: &str = "link";
/// And the tag that marks what is written after it.
const REPORT: &str = "report";

/// Where a tag that marks one stretch of text is, if anywhere.
fn range_of(
    buffer: &gtk::TextBuffer,
    tag: &gtk::TextTag,
) -> Option<(gtk::TextIter, gtk::TextIter)> {
    let mut start = buffer.start_iter();
    if !start.starts_tag(Some(tag)) && !start.forward_to_tag_toggle(Some(tag)) {
        return None;
    }
    let mut end = start;
    end.forward_to_tag_toggle(Some(tag));
    Some((start, end))
}

/// The link at this place in a chat view, if there is one: the text as it
/// was written and the address it opens.
pub fn link_at(iter: &gtk::TextIter) -> Option<(String, String)> {
    iter.tags().iter().find_map(|tag| {
        // Set in `fragments`, always as this type.
        unsafe { tag.data::<(String, String)>(LINK) }.map(|link| unsafe { link.as_ref() }.clone())
    })
}

/// Links in the theme's colour and underlined, or, when they cannot be
/// clicked, nothing to tell them from the text around them.
fn style_links(table: &gtk::TextTagTable, palette: &Palette, clickable: bool) {
    if let Some(tag) = table.lookup("link") {
        if clickable {
            tag.set_foreground(Some(palette.link));
            tag.set_underline(gtk::pango::Underline::Single);
        } else {
            tag.set_foreground(None);
            tag.set_underline(gtk::pango::Underline::None);
        }
    }
}

fn make_tag(name: Option<&str>, colour: Option<&str>, bold: bool) -> gtk::TextTag {
    let tag = gtk::TextTag::new(name);
    if let Some(colour) = colour {
        tag.set_foreground(Some(colour));
    }
    if bold {
        tag.set_weight(700);
    }
    tag
}

/// Sets text apart from the line around it as far as its direction goes
/// (Unicode's first-strong isolate, and its end).
const ISOLATE: &str = "\u{2068}";
const END_ISOLATE: &str = "\u{2069}";

fn insert(buffer: &gtk::TextBuffer, text: &str, tags: &[&str]) {
    let mut end = buffer.end_iter();
    if tags.is_empty() {
        buffer.insert(&mut end, text);
    } else {
        buffer.insert_with_tags_by_name(&mut end, text, tags);
    }
}

/// Pixels between one badge and the next, as Twitch leaves them.
const BADGE_GAP: i32 = 3;

/// How many pixels shorter than a line a badge is drawn.
const BADGE_SHRINK: i32 = 2;

/// Stops GTK's own menu from opening on a view whose text cannot be
/// edited: Cut, Paste, Delete, Undo and Redo can never do anything
/// there, and a menu of nothing but dead entries is worse than none.
pub fn swallow_context_menu(view: &gtk::TextView) {
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(|gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });
    view.add_controller(click);
}

/// `~` broadcaster, `@` moderator, and so on, as in the terminal: what a
/// badge looks like before its picture arrives, or without one.
pub fn badge_symbol(badge: &liveside_proto::model::Badge) -> Option<char> {
    Some(match badge.set_id.as_str() {
        "broadcaster" => '~',
        "moderator" => '@',
        "vip" => '!',
        "subscriber" | "founder" => '%',
        "premium" | "turbo" => '+',
        "bits" => '$',
        "staff" | "admin" => '&',
        _ => return None,
    })
}

/// The name a badge's picture is cached under, in memory and on disk.
/// It is made from the picture's address, not the badge's ids: a channel
/// draws its own subscriber badges, so `subscriber/6` is a cactus in one
/// channel and something else in the next, and keyed by ids every channel
/// showed the first one's (seen 2026-10-27, and kept on disk across
/// restarts). The address is turned into something a file system takes.
pub fn badge_key(badge: &liveside_proto::model::Badge) -> String {
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    };
    let address = badge.image.strip_prefix("https://").unwrap_or(&badge.image);
    if address.is_empty() {
        // Nothing to fetch, so the name is never used for a picture.
        return format!("badge-{}-{}", safe(&badge.set_id), safe(&badge.id));
    }
    format!("badge-{}", safe(address))
}

#[cfg(test)]
fn badge_symbols(badges: &[liveside_proto::model::Badge]) -> String {
    let mut out: String = badges.iter().filter_map(badge_symbol).collect();
    if !out.is_empty() {
        out.push(' ');
    }
    out
}

/// Twitch's own fallback: a colour derived from the name.
fn default_colour(message: &ChatMessage) -> Rgb {
    const PALETTE: [(u8, u8, u8); 8] = [
        (255, 0, 0),
        (0, 0, 255),
        (0, 128, 0),
        (178, 34, 34),
        (255, 127, 80),
        (154, 205, 50),
        (255, 69, 0),
        (46, 139, 87),
    ];
    let sum: u32 = message
        .sender
        .login
        .as_str()
        .bytes()
        .map(u32::from)
        .sum::<u32>();
    let (r, g, b) = PALETTE[(sum as usize) % PALETTE.len()];
    Rgb { r, g, b }
}

/// Local time `HH:MM`, with the offset that was in force at that moment:
/// a line from before a change of summer time keeps the hour it had.
pub(crate) fn clock(time: liveside_proto::Timestamp) -> String {
    match glib::DateTime::from_unix_local(time.0.div_euclid(1000)) {
        Ok(local) => format!("{:02}:{:02}", local.hour(), local.minute()),
        Err(_) => "--:--".into(),
    }
}

/// The height of one line in this view, from Pango's metrics.
fn line_height(view: &gtk::TextView, font: &pango::FontDescription) -> i32 {
    let metrics = view.pango_context().metrics(Some(font), None);
    let height = (metrics.ascent() + metrics.descent()) / pango::SCALE;
    if height > 0 {
        height
    } else {
        crate::emotes::DEFAULT_EMOTE_HEIGHT
    }
}

/// Font metrics are Pango's business; this only says what to use.
pub fn monospace_font() -> pango::FontDescription {
    pango::FontDescription::from_string("Monospace 11")
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::model::Badge;

    fn badge(set_id: &str) -> Badge {
        Badge {
            set_id: set_id.to_owned(),
            id: "1".to_owned(),
            info: String::new(),
            image: String::new(),
            title: String::new(),
        }
    }

    #[test]
    fn badges_become_the_same_symbols_as_in_the_terminal() {
        assert_eq!(badge_symbols(&[]), "");
        assert_eq!(badge_symbols(&[badge("broadcaster")]), "~ ");
        assert_eq!(
            badge_symbols(&[badge("moderator"), badge("subscriber")]),
            "@% "
        );
        // A badge this version does not know is simply not shown.
        assert_eq!(badge_symbols(&[badge("nasa-artemis-ii")]), "");
    }

    #[test]
    fn a_badge_picture_is_cached_under_a_name_a_file_system_accepts() {
        let mut odd = badge("nasa/artemis ii");
        odd.id = "../../etc".to_owned();
        let key = badge_key(&odd);
        assert_eq!(key, "badge-nasa_artemis_ii-______etc");
        assert!(!key.contains('/'), "{key}");
        odd.image = "https://static-cdn.jtvnw.net/badges/v1/../../etc/2".to_owned();
        let key = badge_key(&odd);
        assert!(!key.contains('/') && !key.contains(".."), "{key}");
    }

    #[test]
    fn the_same_badge_in_two_channels_is_two_pictures() {
        let mut here = badge("subscriber");
        here.id = "6".to_owned();
        let mut there = here.clone();
        here.image = "https://static-cdn.jtvnw.net/badges/v1/aaaa-1111/2".to_owned();
        there.image = "https://static-cdn.jtvnw.net/badges/v1/bbbb-2222/2".to_owned();
        assert_ne!(badge_key(&here), badge_key(&there));
        // And the same picture is one, whichever message brings it.
        assert_eq!(badge_key(&here), badge_key(&here.clone()));
    }

    #[test]
    fn everyone_without_a_colour_gets_a_stable_one() {
        use liveside_proto::model::{ChatMessage, MessageKind, User};
        let message = |login: &str| ChatMessage {
            id: "m".into(),
            sender: User {
                id: "1".into(),
                login: login.into(),
                display_name: login.to_owned(),
            },
            color: None,
            badges: Vec::new(),
            fragments: Vec::new(),
            kind: MessageKind::Text,
            bits: None,
            reply: None,
            shared_source: None,
        };
        // The same person keeps their colour between sessions.
        assert_eq!(
            default_colour(&message("alice")),
            default_colour(&message("alice"))
        );
        assert_ne!(
            default_colour(&message("alice")),
            default_colour(&message("bob"))
        );
    }
}
