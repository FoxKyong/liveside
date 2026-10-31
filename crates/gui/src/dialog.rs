//! What every window besides the main one shares.

use gtk::glib;
use gtk::prelude::*;

/// Closes the window on Escape. Every window this client opens besides
/// the main one answers a single question — what is known about a
/// person, what the client does with the reader's data, what a reward
/// costs — and Escape is how a window like that is dismissed
/// everywhere else.
pub fn close_on_escape(window: &gtk::Window) {
    let keys = gtk::EventControllerKey::new();
    // Capture, so a text field inside the window does not swallow it.
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let window = window.clone();
        move |_, key, _, _| match key {
            gtk::gdk::Key::Escape => {
                window.close();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);
}

/// A field for a number in a form. The arrow keys move to the field
/// above or below, as they do from every other field, rather than change
/// the number, which a spin button does by itself: walking down the
/// settings stopped at the first number and began to change it. The
/// mouse wheel likewise scrolls the form rather than the number: a
/// form scrolled with the wheel stopped as soon as a number came under
/// the pointer, and the number began to run instead. The number changes
/// with its − and + buttons, Page Up and Page Down, or by typing it.
pub fn number(min: f64, max: f64, step: f64) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(min, max, step);
    let keys = gtk::EventControllerKey::new();
    // Capture, before the spin button's own handling of the arrows.
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let spin = spin.downgrade();
        move |_, key, _, modifiers| {
            let direction = match key {
                gtk::gdk::Key::Up | gtk::gdk::Key::KP_Up => gtk::DirectionType::Up,
                gtk::gdk::Key::Down | gtk::gdk::Key::KP_Down => gtk::DirectionType::Down,
                _ => return glib::Propagation::Proceed,
            };
            if !modifiers.is_empty() {
                return glib::Propagation::Proceed;
            }
            if let Some(spin) = spin.upgrade() {
                spin.emit_move_focus(direction);
            }
            glib::Propagation::Stop
        }
    });
    spin.add_controller(keys);
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    // Capture again, before the spin button takes the wheel for itself.
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    wheel.connect_scroll({
        let spin = spin.downgrade();
        move |wheel, _, dy| {
            if let Some(spin) = spin.upgrade() {
                scroll_around(&spin, wheel.unit(), dy);
            }
            glib::Propagation::Stop
        }
    });
    spin.add_controller(wheel);
    spin
}

/// Scrolls whatever the widget sits in, as the wheel would have had the
/// widget not been under the pointer: a notch by the same amount GTK
/// scrolls by, a touchpad by the distance moved.
fn scroll_around(widget: &impl IsA<gtk::Widget>, unit: gtk::gdk::ScrollUnit, dy: f64) {
    let Some(scrolled) = widget
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok())
    else {
        return;
    };
    let adjustment = scrolled.vadjustment();
    let distance = match unit {
        gtk::gdk::ScrollUnit::Wheel => dy * adjustment.page_size().powf(2.0 / 3.0),
        _ => dy,
    };
    // The bottom never above the top: clamp panics on a range inside out.
    let bottom = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
    adjustment.set_value((adjustment.value() + distance).clamp(adjustment.lower(), bottom));
}

/// Lets the arrow keys leave a field of several lines at its edges: Down
/// on its last line goes to the field below, Up on its first to the one
/// above. Inside, they move through the text as usual. Walking down the
/// settings otherwise stopped in the first list of rules for good.
pub fn leave_at_edges(view: &gtk::TextView) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let view = view.downgrade();
        move |_, key, _, modifiers| {
            let Some(view) = view.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !modifiers.is_empty() {
                return glib::Propagation::Proceed;
            }
            let buffer = view.buffer();
            // Lines as shown, so a long rule wrapped over two is two: the
            // cursor is on the first or the last when it is as high as
            // the start or as low as the end.
            let row = |iter: &gtk::TextIter| view.iter_location(iter).y();
            let cursor = row(&buffer.iter_at_mark(&buffer.get_insert()));
            let (direction, at_edge) = match key {
                gtk::gdk::Key::Down | gtk::gdk::Key::KP_Down => {
                    (gtk::DirectionType::Down, cursor >= row(&buffer.end_iter()))
                }
                gtk::gdk::Key::Up | gtk::gdk::Key::KP_Up => {
                    (gtk::DirectionType::Up, cursor <= row(&buffer.start_iter()))
                }
                _ => return glib::Propagation::Proceed,
            };
            if !at_edge {
                return glib::Propagation::Proceed;
            }
            view.emit_move_focus(direction);
            glib::Propagation::Stop
        }
    });
    view.add_controller(keys);
}
