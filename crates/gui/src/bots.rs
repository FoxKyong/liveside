//! The user's own bots: a list with how each one stands and a button to
//! start or stop it, the log of the one picked below it, and a form to add
//! or change one. What a bot is and does is in `docs/bots.md`.
//!
//! The window keeps the bots as the engine last reported them and draws
//! the list again when that changes; the engine runs them, so closing the
//! window changes nothing about them.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use liveside_client::bots::{join_command, split_channels, split_command};
use liveside_client::lang::{Strings, fill, from_engine};
use liveside_proto::model::{BotConfig, BotLogLine, BotState, BotStatus};

/// What the window asks for.
pub enum Act {
    Start(String),
    Stop(String),
    /// The whole list, to save and hand to the engine; an error says why
    /// it was not saved.
    Save(Vec<BotConfig>),
}

pub type Acts = Rc<dyn Fn(Act) -> Result<(), String>>;

pub struct BotsWindow {
    window: gtk::Window,
    list: gtk::ListBox,
    empty: gtk::Label,
    log_title: gtk::Label,
    log: gtk::TextView,
    strings: &'static Strings,
    act: Acts,
    open: Rc<Cell<bool>>,
    bots: RefCell<Vec<BotStatus>>,
    /// The bot whose log is shown.
    picked: RefCell<Option<String>>,
    me: RefCell<Weak<BotsWindow>>,
}

impl BotsWindow {
    pub fn open(
        parent: &gtk::ApplicationWindow,
        strings: &'static Strings,
        act: Acts,
        bots: Vec<BotStatus>,
    ) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["boxed-list"])
            .build();
        let empty = gtk::Label::builder()
            .label(strings.bots_empty)
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        let add = gtk::Button::with_label(strings.bots_add);
        add.set_halign(gtk::Align::Start);
        let top = gtk::Box::new(gtk::Orientation::Vertical, 8);
        top.append(&empty);
        top.append(&list);
        top.append(&add);
        let list_scroll = gtk::ScrolledWindow::builder()
            .child(&top)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(160)
            .vexpand(true)
            .build();

        let log_title = gtk::Label::builder()
            .label(strings.bots_log_none)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let log = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .left_margin(6)
            .right_margin(6)
            .top_margin(4)
            .bottom_margin(4)
            .build();
        let log_scroll = gtk::ScrolledWindow::builder()
            .child(&log)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(160)
            .vexpand(true)
            .build();
        let bottom = gtk::Box::new(gtk::Orientation::Vertical, 6);
        bottom.append(&log_title);
        bottom.append(&log_scroll);

        let split = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&list_scroll)
            .end_child(&bottom)
            .position(260)
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();

        let window = gtk::Window::builder()
            .transient_for(parent)
            .modal(false)
            .title(strings.bots_title)
            .default_width(640)
            .default_height(560)
            .child(&split)
            .build();
        crate::dialog::close_on_escape(&window);

        let open = Rc::new(Cell::new(true));
        window.connect_close_request({
            let open = open.clone();
            move |_| {
                open.set(false);
                gtk::glib::Propagation::Proceed
            }
        });

        let this = Rc::new(Self {
            window,
            list,
            empty,
            log_title,
            log,
            strings,
            act,
            open,
            bots: RefCell::new(bots),
            picked: RefCell::new(None),
            me: RefCell::new(Weak::new()),
        });
        *this.me.borrow_mut() = Rc::downgrade(&this);

        this.list.connect_row_selected({
            let me = Rc::downgrade(&this);
            move |_, row| {
                let Some(me) = me.upgrade() else {
                    return;
                };
                let name =
                    row.and_then(|row| row.widget_name().strip_prefix("bot:").map(str::to_owned));
                *me.picked.borrow_mut() = name;
                me.draw_log();
            }
        });
        add.connect_clicked({
            let me = Rc::downgrade(&this);
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.edit(None);
                }
            }
        });

        this.draw();
        this.window.present();
        this
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// The whole list, as the engine has it now.
    pub fn show(&self, bots: Vec<BotStatus>) {
        *self.bots.borrow_mut() = bots;
        self.draw();
    }

    /// How one bot stands now.
    pub fn state(&self, name: &str, state: &BotState) {
        if let Some(bot) = self
            .bots
            .borrow_mut()
            .iter_mut()
            .find(|bot| bot.config.name == name)
        {
            bot.state = state.clone();
        }
        self.draw_list();
    }

    /// A line for a bot's log.
    pub fn log_line(&self, name: &str, line: &BotLogLine) {
        if let Some(bot) = self
            .bots
            .borrow_mut()
            .iter_mut()
            .find(|bot| bot.config.name == name)
        {
            bot.log.push(line.clone());
        }
        if self.picked.borrow().as_deref() == Some(name) {
            self.append_log(line);
        }
    }

    fn draw(&self) {
        self.draw_list();
        self.draw_log();
    }

    fn draw_list(&self) {
        let picked = self.picked.borrow().clone();
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let bots = self.bots.borrow().clone();
        self.empty.set_visible(bots.is_empty());
        self.list.set_visible(!bots.is_empty());
        for bot in &bots {
            let row = self.row(bot);
            self.list.append(&row);
            if picked.as_deref() == Some(bot.config.name.as_str()) {
                self.list.select_row(Some(&row));
            }
        }
        // Selecting while drawing asks for the log again; the picked one
        // is still the same one.
        *self.picked.borrow_mut() = picked;
    }

    fn row(&self, bot: &BotStatus) -> gtk::ListBoxRow {
        let strings = self.strings;
        let name = gtk::Label::builder()
            .label(&bot.config.name)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let (said, class) = state_text(strings, &bot.state);
        let state = gtk::Label::builder()
            .label(said)
            .xalign(0.0)
            .css_classes([class])
            .build();
        let first = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        first.append(&name);
        first.append(&state);
        let mut details = vec![join_command(&bot.config.command)];
        details.push(if bot.config.channels.is_empty() {
            strings.bots_all_channels.to_owned()
        } else {
            bot.config
                .channels
                .iter()
                .map(|c| format!("#{c}"))
                .collect::<Vec<_>>()
                .join(" ")
        });
        if bot.config.autostart {
            details.push(strings.bots_autostart_note.to_owned());
        }
        // What differs from the defaults, so a bot set apart is seen to be.
        if bot.config.rate != BotConfig::DEFAULT_RATE {
            let rate = bot.config.rate.to_string();
            details.push(fill(strings.bots_rate_shown, &[("rate", &rate)]));
        }
        if bot.config.hear_bots {
            details.push(strings.bots_hears_bots.to_owned());
        }
        if bot.config.hear_ignored {
            details.push(strings.bots_hears_ignored.to_owned());
        }
        let details = gtk::Label::builder()
            .label(details.join(" · "))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .build();
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.append(&first);
        text.append(&details);

        // Waiting for another Liveside to let it go counts as wanted
        // running: ⏹ stops the wait.
        let running = matches!(bot.state, BotState::Running | BotState::Elsewhere);
        let go = gtk::Button::builder()
            .icon_name(if running {
                "media-playback-stop-symbolic"
            } else {
                "media-playback-start-symbolic"
            })
            .tooltip_text(if running {
                strings.bots_stop
            } else {
                strings.bots_start
            })
            .valign(gtk::Align::Center)
            .build();
        let edit = gtk::Button::builder()
            .label(strings.bots_edit)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let remove = gtk::Button::builder()
            .label(strings.bots_remove)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();

        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.set_margin_start(8);
        line.set_margin_end(8);
        line.set_margin_top(6);
        line.set_margin_bottom(6);
        line.append(&text);
        line.append(&go);
        line.append(&edit);
        line.append(&remove);
        let row = gtk::ListBoxRow::builder().child(&line).build();
        row.set_widget_name(&format!("bot:{}", bot.config.name));

        let me = self.me.borrow().clone();
        let bot_name = bot.config.name.clone();
        go.connect_clicked({
            let (me, bot_name) = (me.clone(), bot_name.clone());
            move |_| {
                if let Some(me) = me.upgrade() {
                    let act = if running {
                        Act::Stop(bot_name.clone())
                    } else {
                        Act::Start(bot_name.clone())
                    };
                    let _ = (me.act)(act);
                }
            }
        });
        edit.connect_clicked({
            let (me, bot_name) = (me.clone(), bot_name.clone());
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.edit(Some(&bot_name));
                }
            }
        });
        remove.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.confirm_remove(&bot_name);
            }
        });
        row
    }

    fn draw_log(&self) {
        let buffer = self.log.buffer();
        buffer.set_text("");
        let picked = self.picked.borrow().clone();
        let bots = self.bots.borrow();
        let Some(bot) = picked
            .as_deref()
            .and_then(|name| bots.iter().find(|bot| bot.config.name == name))
        else {
            self.log_title.set_label(self.strings.bots_log_none);
            return;
        };
        self.log_title
            .set_label(&fill(self.strings.bots_log, &[("bot", &bot.config.name)]));
        let lines = bot.log.clone();
        drop(bots);
        for line in &lines {
            self.append_log(line);
        }
    }

    fn append_log(&self, line: &BotLogLine) {
        let buffer = self.log.buffer();
        let text = from_engine(self.strings, &line.code, &line.args, &line.text);
        let mut end = buffer.end_iter();
        if buffer.char_count() > 0 {
            buffer.insert(&mut end, "\n");
        }
        buffer.insert(
            &mut end,
            &format!("{} {text}", crate::chat::clock(line.time)),
        );
        // The newest line in view, as in a terminal.
        let mark = buffer.create_mark(None, &buffer.end_iter(), false);
        self.log.scroll_mark_onscreen(&mark);
        buffer.delete_mark(&mark);
    }

    fn configs(&self) -> Vec<BotConfig> {
        self.bots
            .borrow()
            .iter()
            .map(|bot| bot.config.clone())
            .collect()
    }

    fn confirm_remove(&self, name: &str) {
        let strings = self.strings;
        let dialog = gtk::AlertDialog::builder()
            .message(fill(strings.bots_confirm_remove, &[("bot", name)]))
            .buttons([strings.cancel, strings.bots_remove])
            .cancel_button(0)
            .default_button(0)
            .build();
        let me = self.me.borrow().clone();
        let name = name.to_owned();
        dialog.choose(
            Some(&self.window),
            gtk::gio::Cancellable::NONE,
            move |answer| {
                let Some(me) = me.upgrade() else {
                    return;
                };
                if answer == Ok(1) {
                    let list: Vec<BotConfig> = me
                        .configs()
                        .into_iter()
                        .filter(|bot| bot.name != name)
                        .collect();
                    let _ = (me.act)(Act::Save(list));
                }
            },
        );
    }

    /// The form for a new bot, or for changing the one called `name`.
    fn edit(&self, name: Option<&str>) {
        let strings = self.strings;
        let current = name.and_then(|name| self.configs().into_iter().find(|bot| bot.name == name));
        let field = |text: &str, hint: &str| {
            gtk::Entry::builder()
                .text(text)
                .placeholder_text(hint)
                .hexpand(true)
                .width_chars(36)
                .build()
        };
        let shown = current.clone().unwrap_or_default();
        let name_field = field(&shown.name, "");
        let command_field = field(&join_command(&shown.command), strings.bot_command_hint);
        let dir_field = field(&shown.dir, strings.bot_dir_hint);
        let channels_field = field(&shown.channels.join(" "), strings.bot_channels_hint);
        let autostart = gtk::CheckButton::builder()
            .label(strings.bot_autostart)
            .active(shown.autostart)
            .build();
        let number = |value: u32, most: u32| {
            let spin = crate::dialog::number(1.0, f64::from(most), 1.0);
            spin.set_value(f64::from(value));
            spin.set_halign(gtk::Align::Start);
            spin
        };
        let rate = number(shown.rate, BotConfig::MAX_RATE);
        let stop_after = number(shown.stop_after, BotConfig::MAX_STOP_AFTER);
        stop_after.set_tooltip_text(Some(strings.bot_stop_after_note));
        let note = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(48)
                .css_classes(["dim-label"])
                .build()
        };
        let hear_bots = gtk::CheckButton::builder()
            .label(strings.bot_hear_bots)
            .active(shown.hear_bots)
            .build();
        let hear_ignored = gtk::CheckButton::builder()
            .label(strings.bot_hear_ignored)
            .active(shown.hear_ignored)
            .build();
        let problem = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["error"])
            .visible(false)
            .build();

        let grid = gtk::Grid::builder()
            .row_spacing(8)
            .column_spacing(12)
            .build();
        for (row, (label, widget)) in [
            (strings.bot_name, name_field.upcast_ref::<gtk::Widget>()),
            (strings.bot_command, command_field.upcast_ref()),
            (strings.bot_dir, dir_field.upcast_ref()),
            (strings.bot_channels, channels_field.upcast_ref()),
        ]
        .into_iter()
        .enumerate()
        {
            let label = gtk::Label::builder().label(label).xalign(0.0).build();
            grid.attach(&label, 0, row as i32, 1, 1);
            grid.attach(widget, 1, row as i32, 1, 1);
        }
        let label = |text: &str| gtk::Label::builder().label(text).xalign(0.0).build();
        grid.attach(&label(strings.bot_rate), 0, 4, 1, 1);
        grid.attach(&rate, 1, 4, 1, 1);
        grid.attach(&note(strings.bot_rate_note), 1, 5, 1, 1);
        grid.attach(&label(strings.bot_stop_after), 0, 6, 1, 1);
        grid.attach(&stop_after, 1, 6, 1, 1);
        grid.attach(&autostart, 1, 7, 1, 1);
        grid.attach(&hear_bots, 1, 8, 1, 1);
        grid.attach(&note(strings.bot_hear_bots_note), 1, 9, 1, 1);
        grid.attach(&hear_ignored, 1, 10, 1, 1);

        let cancel = gtk::Button::with_label(strings.cancel);
        let save = gtk::Button::builder()
            .label(strings.bot_save)
            .css_classes(["suggested-action"])
            .build();
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&save);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
        outer.set_margin_start(16);
        outer.set_margin_end(16);
        outer.set_margin_top(16);
        outer.set_margin_bottom(16);
        outer.append(&grid);
        outer.append(&problem);
        outer.append(&buttons);

        let title = match &current {
            Some(bot) => fill(strings.bot_edit_title, &[("bot", &bot.name)]),
            None => strings.bot_new_title.to_owned(),
        };
        let form = gtk::Window::builder()
            .transient_for(&self.window)
            .modal(true)
            .title(title)
            .child(&outer)
            .build();
        crate::dialog::close_on_escape(&form);
        cancel.connect_clicked({
            let form = form.clone();
            move |_| form.close()
        });

        let me = self.me.borrow().clone();
        let was = current.map(|bot| bot.name);
        let submit = {
            let form = form.clone();
            let (name_field, command_field, dir_field, channels_field) = (
                name_field.clone(),
                command_field.clone(),
                dir_field.clone(),
                channels_field.clone(),
            );
            move || {
                let Some(me) = me.upgrade() else {
                    return;
                };
                let say = |text: String| {
                    problem.set_label(&text);
                    problem.set_visible(true);
                };
                let name = name_field.text().trim().to_owned();
                if name.is_empty() {
                    return say(strings.bot_no_name.to_owned());
                }
                let command = match split_command(&command_field.text()) {
                    Ok(words) if words.is_empty() => {
                        return say(strings.bot_no_command.to_owned());
                    }
                    Ok(words) => words,
                    Err(e) => return say(fill(strings.bot_bad_command, &[("error", &e)])),
                };
                let bot = BotConfig {
                    name,
                    command,
                    dir: dir_field.text().trim().to_owned(),
                    channels: split_channels(&channels_field.text()),
                    autostart: autostart.is_active(),
                    rate: rate.value_as_int().max(1) as u32,
                    stop_after: stop_after.value_as_int().max(1) as u32,
                    hear_bots: hear_bots.is_active(),
                    hear_ignored: hear_ignored.is_active(),
                };
                let mut list = me.configs();
                match was
                    .as_deref()
                    .and_then(|was| list.iter().position(|b| b.name == was))
                {
                    Some(at) => list[at] = bot,
                    None => list.push(bot),
                }
                match (me.act)(Act::Save(list)) {
                    Ok(()) => form.close(),
                    Err(e) => say(fill(strings.bot_save_failed, &[("error", &e)])),
                }
            }
        };
        save.connect_clicked({
            let submit = submit.clone();
            move |_| submit()
        });
        for entry in [&name_field, &command_field, &dir_field, &channels_field] {
            let submit = submit.clone();
            entry.connect_activate(move |_| submit());
        }
        form.present();
        name_field.grab_focus();
    }
}

/// What a bot's state is called, and the style it is shown in.
pub fn state_text(strings: &'static Strings, state: &BotState) -> (String, &'static str) {
    let class = match state {
        BotState::Running => "success",
        BotState::Elsewhere => "warning",
        state if liveside_client::bots::went_wrong(state) => "error",
        _ => "dim-label",
    };
    (liveside_client::bots::state_said(strings, state), class)
}
