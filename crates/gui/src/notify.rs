//! Notifications and sounds in the window: what `liveside_client::notify`
//! decides is worth one, raised on the desktop and played aloud.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use liveside_client::config::NotifyConfig;
use liveside_client::lang::{Strings, fill};
use liveside_client::notify::{Alert, Clicked, Desktop, Kind, Notifier, PlayError, Seen};
use liveside_proto::Event;

/// Something to say in the status view, from the notifications.
pub type Say = Box<dyn Fn(String)>;

pub struct Notifications {
    notifier: RefCell<Notifier>,
    desktop: Desktop,
    /// Kinds already said to have no sound, so that is said once.
    silent: RefCell<HashSet<Kind>>,
    /// Files that would not play, said once each and not tried again
    /// until the settings change.
    broken: Rc<RefCell<HashSet<PathBuf>>>,
    /// Whether it has been said that there is nothing to play sounds with.
    no_player: Rc<Cell<bool>>,
    /// Where problems are said: the status view.
    say: RefCell<Option<Rc<Say>>>,
}

impl Notifications {
    /// Starts the desktop side. `failed` hears, from another thread, why
    /// desktop notifications cannot be shown, once; `clicked`, from
    /// another thread too, where a notification the reader clicked leads.
    pub fn new(
        config: NotifyConfig,
        failed: impl FnOnce(String) + Send + 'static,
        clicked: Clicked,
        text: &'static Strings,
    ) -> Self {
        Self {
            notifier: RefCell::new(Notifier::new(config)),
            desktop: Desktop::start(failed, clicked, text.notify_open),
            silent: RefCell::new(HashSet::new()),
            broken: Rc::new(RefCell::new(HashSet::new())),
            no_player: Rc::new(Cell::new(false)),
            say: RefCell::new(None),
        }
    }

    /// Where to say a sound that is missing or will not play.
    pub fn set_say(&self, say: Say) {
        *self.say.borrow_mut() = Some(Rc::new(say));
    }

    pub fn set_config(&self, config: NotifyConfig) {
        // A file set or changed just now gets its chance again.
        self.silent.borrow_mut().clear();
        self.broken.borrow_mut().clear();
        self.no_player.set(false);
        self.notifier.borrow_mut().set_config(config);
    }

    pub fn know_channel(&self, channel: &liveside_proto::model::Channel, live: bool) {
        self.notifier.borrow_mut().know_channel(channel, live);
    }

    /// Looks at an event and raises what it is worth, given what is on
    /// screen.
    pub fn consider(&self, event: &Event, seen: &Seen, text: &'static Strings) {
        let Some(alert) = self.notifier.borrow_mut().consider(event, text) else {
            return;
        };
        if !self.notifier.borrow().wanted(&alert, seen) {
            return;
        }
        self.raise(alert, text);
    }

    fn raise(&self, alert: Alert, text: &'static Strings) {
        let kind = alert.kind;
        let silent = alert.silent;
        if self.notifier.borrow().config().desktop && !alert.quiet {
            let close_after = self.notifier.borrow().config().close_after;
            self.desktop.show(alert, close_after);
        }
        if !silent && self.notifier.borrow_mut().sound_now(kind) {
            self.play(kind, text);
        }
    }

    fn play(&self, kind: Kind, text: &'static Strings) {
        let file = self.notifier.borrow().sound_file(kind);
        let Some(file) = file else {
            if self.silent.borrow_mut().insert(kind) {
                self.tell(fill(
                    text.notify_sound_missing,
                    &[("kind", kind.label(text))],
                ));
            }
            return;
        };
        if self.broken.borrow().contains(&file) || self.no_player.get() {
            return;
        }
        // The player says how it went from its own thread; the answer is
        // said here, once: a broken file would otherwise fill the status
        // view with every mention.
        let (tx, rx) = async_channel::bounded(1);
        liveside_client::notify::play(&file, move |error| {
            let _ = tx.send_blocking(error);
        });
        let say = self.say.borrow().clone();
        let broken = self.broken.clone();
        let no_player = self.no_player.clone();
        gtk::glib::spawn_future_local(async move {
            let Ok(error) = rx.recv().await else {
                return;
            };
            let said = match error {
                PlayError::NoPlayer if !no_player.replace(true) => text.notify_no_player.to_owned(),
                PlayError::Failed(why) if broken.borrow_mut().insert(file.clone()) => fill(
                    text.notify_sound_failed,
                    &[("file", &file.display().to_string()), ("error", &why)],
                ),
                _ => return,
            };
            if let Some(say) = say {
                say(said);
            }
        });
    }

    fn tell(&self, said: String) {
        if let Some(say) = self.say.borrow().as_ref() {
            say(said);
        }
    }
}
