//! The list of emotes to pick from, beside the line one types into.
//!
//! A channel's custom emotes are the ones nobody can spell: this shows
//! them as pictures, by section (recently used, the channel's own, other
//! channels', Twitch's, the third-party providers'), with a search over
//! their codes. Clicking one writes its code into the line and leaves the
//! list open for the next, as Twitch's own does.
//!
//! A large channel has hundreds of 7TV emotes, so a picture is asked for
//! only once its emote comes into the visible part of the list.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use liveside_client::lang::Strings;
use liveside_proto::model::EmoteRef;

use crate::emotes::Emotes;

/// How tall an emote is in the list, in pixels.
const SIZE: i32 = 28;

/// How far outside the visible part a picture is asked for already, so
/// that scrolling finds it there.
const AHEAD: f32 = 200.0;

/// How long scrolling has to stop before pictures are asked for.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// One section of the list: its heading and its emotes.
pub struct Section {
    pub title: String,
    pub emotes: Vec<EmoteRef>,
}

/// One emote in the list, with what searching needs.
struct Shown {
    child: gtk::FlowBoxChild,
    code: String,
    emote: EmoteRef,
}

pub struct Picker {
    pub button: gtk::MenuButton,
    popover: gtk::Popover,
    search: gtk::SearchEntry,
    content: gtk::Box,
    scroll: gtk::ScrolledWindow,
    note: gtk::Label,
    none: gtk::Label,
    emotes: Rc<Emotes>,
    entry: gtk::Entry,
    /// Emotes whose picture has not been asked for yet.
    waiting: RefCell<Vec<(glib::WeakRef<gtk::Button>, EmoteRef)>>,
    /// Each section's heading and box, and its emotes.
    shown: RefCell<Vec<(gtk::Label, gtk::FlowBox, Vec<Shown>)>>,
    /// The wait for scrolling to come to rest.
    settling: RefCell<Option<glib::SourceId>>,
    picked: Box<dyn Fn(&EmoteRef)>,
}

impl Picker {
    /// The button and its list, for `entry`. `opened` runs as the list is
    /// shown, to fill it; `picked` after an emote has been written.
    pub fn new(
        entry: &gtk::Entry,
        emotes: Rc<Emotes>,
        strings: &'static Strings,
        opened: impl Fn() + 'static,
        picked: impl Fn(&EmoteRef) + 'static,
    ) -> Rc<Self> {
        let popover = gtk::Popover::new();
        let button = gtk::MenuButton::builder()
            .icon_name("face-smile-symbolic")
            .tooltip_text(strings.emotes_open)
            .direction(gtk::ArrowType::Up)
            .popover(&popover)
            .build();

        let search = gtk::SearchEntry::builder()
            .placeholder_text(strings.emotes_search)
            .hexpand(true)
            .build();
        let emoji = gtk::Button::builder()
            .label("😀")
            .tooltip_text(strings.emotes_emoji)
            .build();
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&search);
        header.append(&emoji);

        let note = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(40)
            .css_classes(["dim-label"])
            .visible(false)
            .build();
        let none = gtk::Label::builder()
            .label(strings.emotes_none)
            .css_classes(["dim-label"])
            .visible(false)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(320)
            .min_content_width(380)
            .vexpand(true)
            .build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.append(&header);
        column.append(&note);
        column.append(&scroll);
        column.append(&none);
        popover.set_child(Some(&column));

        let picker = Rc::new(Self {
            button,
            popover: popover.clone(),
            search: search.clone(),
            content,
            scroll: scroll.clone(),
            note,
            none,
            emotes,
            entry: entry.clone(),
            waiting: RefCell::default(),
            shown: RefCell::default(),
            settling: RefCell::default(),
            picked: Box::new(picked),
        });

        popover.connect_show({
            let picker = Rc::downgrade(&picker);
            move |_| {
                let Some(picker) = picker.upgrade() else {
                    return;
                };
                picker.search.set_text("");
                opened();
                picker.search.grab_focus();
                picker.load_visible_soon();
            }
        });
        // Back to typing once the list is closed.
        popover.connect_closed({
            let entry = entry.clone();
            move |_| {
                entry.grab_focus_without_selecting();
            }
        });
        // Pictures are asked for where the list comes to rest: scrolling
        // through hundreds of emotes would otherwise queue every one of
        // them, and the ones in sight would come last.
        scroll.vadjustment().connect_value_changed({
            let picker = Rc::downgrade(&picker);
            move |_| {
                let Some(picker) = picker.upgrade() else {
                    return;
                };
                if let Some(waiting) = picker.settling.borrow_mut().take() {
                    waiting.remove();
                }
                let later = Rc::downgrade(&picker);
                let id = glib::timeout_add_local_once(SETTLE, move || {
                    if let Some(picker) = later.upgrade() {
                        picker.settling.borrow_mut().take();
                        picker.load_visible();
                    }
                });
                *picker.settling.borrow_mut() = Some(id);
            }
        });
        search.connect_search_changed({
            let picker = Rc::downgrade(&picker);
            move |search| {
                if let Some(picker) = picker.upgrade() {
                    picker.filter(&search.text());
                    picker.load_visible_soon();
                }
            }
        });
        // Enter takes the first emote the search left.
        search.connect_activate({
            let picker = Rc::downgrade(&picker);
            move |_| {
                let Some(picker) = picker.upgrade() else {
                    return;
                };
                let first = picker
                    .shown
                    .borrow()
                    .iter()
                    .flat_map(|(_, _, emotes)| emotes)
                    .find(|shown| shown.child.is_visible())
                    .map(|shown| shown.emote.clone());
                if let Some(emote) = first {
                    picker.pick(&emote);
                }
            }
        });

        // Unicode emoji come from GTK's own chooser.
        let chooser = gtk::EmojiChooser::new();
        chooser.set_parent(&emoji);
        chooser.connect_emoji_picked({
            let entry = entry.clone();
            move |_, emoji| write_word(&entry, emoji)
        });
        emoji.connect_clicked(move |_| chooser.popup());

        picker
    }

    /// Whether the list is open: an answer that arrives then goes in.
    pub fn is_open(&self) -> bool {
        self.popover.is_visible()
    }

    /// Says something instead of a list: that it is on its way, or that
    /// there is no channel to list.
    pub fn show_note(&self, text: &str) {
        self.clear();
        self.note.set_text(text);
        self.note.set_visible(true);
    }

    /// Puts the sections in, with `note` above them when there is one.
    pub fn fill(self: &Rc<Self>, sections: Vec<Section>, note: Option<&str>) {
        self.clear();
        self.note.set_text(note.unwrap_or_default());
        self.note.set_visible(note.is_some());
        let mut shown = Vec::new();
        for section in sections.into_iter().filter(|s| !s.emotes.is_empty()) {
            let heading = gtk::Label::builder()
                .label(&section.title)
                .xalign(0.0)
                .css_classes(["heading"])
                .build();
            let flow = gtk::FlowBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .max_children_per_line(12)
                .row_spacing(2)
                .column_spacing(2)
                .valign(gtk::Align::Start)
                .build();
            let mut emotes = Vec::new();
            for emote in section.emotes {
                let button = self.button_for(&emote);
                flow.insert(&button, -1);
                let Some(child) = button.parent().and_downcast::<gtk::FlowBoxChild>() else {
                    continue;
                };
                // Focus goes to the button, not to the box around it.
                child.set_focusable(false);
                self.waiting
                    .borrow_mut()
                    .push((button.downgrade(), emote.clone()));
                emotes.push(Shown {
                    child,
                    code: emote.code.to_lowercase(),
                    emote,
                });
            }
            self.content.append(&heading);
            self.content.append(&flow);
            shown.push((heading, flow, emotes));
        }
        *self.shown.borrow_mut() = shown;
        self.filter(&self.search.text());
        self.load_visible_soon();
    }

    fn clear(&self) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        self.shown.borrow_mut().clear();
        self.waiting.borrow_mut().clear();
        self.none.set_visible(false);
    }

    /// A button showing the emote's code until its picture comes.
    fn button_for(self: &Rc<Self>, emote: &EmoteRef) -> gtk::Button {
        let code = gtk::Label::builder()
            .label(&emote.code)
            .max_width_chars(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption"])
            .build();
        // Every emote in a square of the same size: the list is a grid,
        // and one wide 7TV emote would otherwise widen every column.
        let button = gtk::Button::builder()
            .child(&code)
            .tooltip_text(&emote.code)
            .css_classes(["flat"])
            .width_request(SIZE + 12)
            .height_request(SIZE + 12)
            .build();
        button.connect_clicked({
            let picker = Rc::downgrade(self);
            let emote = emote.clone();
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.pick(&emote);
                }
            }
        });
        button
    }

    fn pick(&self, emote: &EmoteRef) {
        write_word(&self.entry, &emote.code);
        (self.picked)(emote);
    }

    /// Shows only the emotes whose code has `query` in it, and the
    /// sections that still have any.
    fn filter(&self, query: &str) {
        let query = query.trim().to_lowercase();
        let mut any = false;
        for (heading, flow, emotes) in self.shown.borrow().iter() {
            let mut here = false;
            for shown in emotes {
                let visible = query.is_empty() || shown.code.contains(&query);
                shown.child.set_visible(visible);
                here |= visible;
            }
            heading.set_visible(here);
            flow.set_visible(here);
            any |= here;
        }
        self.none
            .set_visible(!any && !self.shown.borrow().is_empty());
    }

    /// Asks for the pictures in sight once GTK has laid the list out.
    fn load_visible_soon(self: &Rc<Self>) {
        let picker = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(picker) = picker.upgrade() {
                picker.load_visible();
            }
        });
    }

    /// Asks for the pictures of the emotes in the visible part of the
    /// list, and a little beyond.
    fn load_visible(&self) {
        let height = self.scroll.height() as f32;
        if height <= 0.0 {
            return;
        }
        let mut asked = Vec::new();
        self.waiting.borrow_mut().retain(|(button, emote)| {
            let Some(button) = button.upgrade() else {
                return false;
            };
            if !button.is_drawable() {
                return true;
            }
            let Some(bounds) = button.compute_bounds(&self.scroll) else {
                return true;
            };
            let near = bounds.y() + bounds.height() > -AHEAD && bounds.y() < height + AHEAD;
            if near {
                asked.push((button, emote.clone()));
            }
            !near
        });
        for (button, emote) in asked {
            // The size chat uses, about as tall as the list's emotes: a
            // picture is never drawn smaller than it is. A Twitch emote
            // comes in the still version its maker drew: the first frame
            // of a moving one can be empty, which left holes in the list.
            let (url, key) = match emote.provider {
                liveside_proto::model::EmoteProvider::Twitch => (
                    format!(
                        "https://static-cdn.jtvnw.net/emoticons/v2/{}/static/dark/1.0",
                        emote.id
                    ),
                    format!("Twitch-{}-static", emote.id),
                ),
                _ => {
                    let Some(url) = emote.image_url(1) else {
                        continue;
                    };
                    // The same key shares what chat has fetched already.
                    (url, format!("{:?}-{}", emote.provider, emote.id))
                }
            };
            let weak = button.downgrade();
            // The first frame, fitted into the square: hundreds of emotes
            // moving at once would be a lot to draw, and a list to pick
            // from only has to show which emote it is.
            self.emotes.request_url(key, url, move |animation| {
                if let Some(button) = weak.upgrade() {
                    let image = gtk::Image::from_paintable(Some(animation.still()));
                    image.set_pixel_size(SIZE);
                    button.set_child(Some(&image));
                }
            });
        }
    }
}

/// Writes `word` where the cursor is in `entry`, with a space before it
/// when it would otherwise run into the word before, and one after.
fn write_word(entry: &gtk::Entry, word: &str) {
    let text = entry.text();
    let cursor = entry.position().max(0) as usize;
    let before: String = text.chars().take(cursor).collect();
    let after: String = text.chars().skip(cursor).collect();
    let mut insert = String::new();
    if !before.is_empty() && !before.ends_with(' ') {
        insert.push(' ');
    }
    insert.push_str(word);
    if !after.starts_with(' ') {
        insert.push(' ');
    }
    let mut at = cursor as i32;
    entry.insert_text(&insert, &mut at);
    entry.set_position(at);
}
