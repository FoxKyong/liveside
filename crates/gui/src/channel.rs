//! What the channel says about itself: the form behind `/title` and
//! `/game`, with the fields those two never reached — the language, the
//! tags, the stream delay and Twitch's content labels.
//!
//! It is a form rather than commands because these are set together and
//! rarely: before going live, once.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::Strings;
use liveside_proto::model::ChannelInfo;

/// Twitch's own labels, in its own spelling, with a readable name.
const LABELS: &[(&str, &str)] = &[
    ("DebatedSocialIssuesAndPolitics", "Politics"),
    ("DrugsIntoxication", "Drugs"),
    ("SexualThemes", "Sexual themes"),
    ("ViolentGraphic", "Violence"),
    ("Gambling", "Gambling"),
    ("ProfanityVulgarity", "Profanity"),
];

/// What the language field says it wants. Twitch takes any two-letter
/// code it supports, or `other`, so the field is typed rather than
/// picked from a list of a hundred.
const LANGUAGE_HINT: &str = "cs, sk, en, de, pl, … or other";

pub struct ChannelWindow {
    window: gtk::Window,
    title: gtk::Entry,
    game: gtk::Entry,
    language: gtk::Entry,
    tags: gtk::Entry,
    delay: gtk::SpinButton,
    labels: Vec<(String, gtk::CheckButton)>,
    /// What was last drawn, so saving sends only what changed: Twitch
    /// takes a partial change and the rest is left alone.
    shown: RefCell<ChannelInfo>,
}

impl ChannelWindow {
    pub fn open(
        parent: &gtk::ApplicationWindow,
        strings: &'static Strings,
        save: Rc<dyn Fn(ChannelInfo)>,
    ) -> Rc<Self> {
        let rows = gtk::Grid::builder()
            .row_spacing(8)
            .column_spacing(12)
            .margin_start(16)
            .margin_end(16)
            .margin_top(16)
            .margin_bottom(16)
            .build();
        let label = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build()
        };

        let title = gtk::Entry::builder().hexpand(true).build();
        let game = gtk::Entry::builder().hexpand(true).build();
        let language = gtk::Entry::builder()
            .placeholder_text(LANGUAGE_HINT)
            .hexpand(true)
            .build();
        let tags = gtk::Entry::builder()
            .hexpand(true)
            .placeholder_text(strings.channel_tags_help)
            .build();
        let delay = crate::dialog::number(0.0, 900.0, 15.0);
        delay.set_tooltip_text(Some(strings.channel_delay_help));

        rows.attach(&label(strings.channel_title), 0, 0, 1, 1);
        rows.attach(&title, 1, 0, 1, 1);
        rows.attach(&label(strings.channel_game), 0, 1, 1, 1);
        rows.attach(&game, 1, 1, 1, 1);
        rows.attach(&label(strings.channel_language), 0, 2, 1, 1);
        rows.attach(&language, 1, 2, 1, 1);
        rows.attach(&label(strings.channel_tags), 0, 3, 1, 1);
        rows.attach(&tags, 1, 3, 1, 1);
        rows.attach(&label(strings.channel_delay), 0, 4, 1, 1);
        rows.attach(&delay, 1, 4, 1, 1);

        // The labels are a set of switches, and Twitch wants all of them
        // on every change: what is unticked is turned off.
        let boxes = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let mut labels = Vec::new();
        for (id, name) in LABELS {
            let check = gtk::CheckButton::with_label(name);
            boxes.append(&check);
            labels.push(((*id).to_owned(), check));
        }
        rows.attach(&label(strings.channel_labels), 0, 5, 1, 1);
        rows.attach(&boxes, 1, 5, 1, 1);

        let window = gtk::Window::builder()
            .transient_for(parent)
            .modal(false)
            .title(strings.channel_window)
            .default_width(520)
            .build();
        crate::dialog::close_on_escape(&window);

        let button = gtk::Button::with_label(strings.channel_save);
        button.add_css_class("suggested-action");
        button.set_halign(gtk::Align::End);
        rows.attach(&button, 1, 6, 1, 1);
        window.set_child(Some(&rows));

        let form = Rc::new(Self {
            window: window.clone(),
            title,
            game,
            language,
            tags,
            delay,
            labels,
            shown: RefCell::new(ChannelInfo::default()),
        });
        button.connect_clicked({
            let form = form.clone();
            move |_| save(form.collect())
        });
        window.present();
        form
    }

    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    /// Fills the form from what the engine says the channel is now.
    pub fn show(&self, info: &ChannelInfo) {
        self.title.set_text(&info.title);
        self.game.set_text(&info.game);
        self.language.set_text(&info.language);
        self.tags.set_text(&info.tags.join(", "));
        self.delay.set_value(f64::from(info.delay));
        for (id, check) in &self.labels {
            check.set_active(info.labels.iter().any(|set| set == id));
        }
        *self.shown.borrow_mut() = info.clone();
    }

    /// What the form says now.
    fn collect(&self) -> ChannelInfo {
        ChannelInfo {
            title: self.title.text().trim().to_string(),
            game: self.game.text().trim().to_string(),
            language: self.language.text().trim().to_string(),
            tags: self
                .tags
                .text()
                .split(',')
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty())
                .collect(),
            delay: self.delay.value() as u32,
            labels: self
                .labels
                .iter()
                .filter(|(_, check)| check.is_active())
                .map(|(id, _)| id.clone())
                .collect(),
        }
    }

    /// What of it differs from what was drawn: a change Twitch is not
    /// told about is a field it leaves alone, which is what a form that
    /// only edited the title should do.
    pub fn changes(&self, now: &ChannelInfo) -> ChannelChanges {
        let before = self.shown.borrow();
        ChannelChanges {
            title: (now.title != before.title && !now.title.is_empty()).then(|| now.title.clone()),
            game: (now.game != before.game && !now.game.is_empty()).then(|| now.game.clone()),
            language: (now.language != before.language && !now.language.is_empty())
                .then(|| now.language.clone()),
            tags: (now.tags != before.tags).then(|| now.tags.clone()),
            delay: (now.delay != before.delay).then_some(now.delay),
            labels: (now.labels != before.labels).then(|| now.labels.clone()),
        }
    }
}

/// Only the fields that were edited.
pub struct ChannelChanges {
    pub title: Option<String>,
    pub game: Option<String>,
    pub language: Option<String>,
    pub tags: Option<Vec<String>>,
    pub delay: Option<u32>,
    pub labels: Option<Vec<String>>,
}
