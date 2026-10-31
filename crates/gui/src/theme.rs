//! Light and dark colours for the window.
//!
//! GTK repaints its own widgets when asked for a dark theme, but the chat
//! is drawn with colours of ours: the time, notices, badges, emote codes,
//! and the colour a viewer chose for their name. Those have to turn round
//! with it, or the window ends up dark with a chat written for white.

use gtk::glib;
use gtk::glib::object::ObjectExt;
use liveside_client::config::Theme;
use liveside_proto::ids::Rgb;

/// The colours the chat is drawn with, one set per background.
pub struct Palette {
    pub dark: bool,
    pub time: &'static str,
    pub notice: &'static str,
    pub warning: &'static str,
    pub error: &'static str,
    pub event: &'static str,
    pub badge: &'static str,
    pub emote: &'static str,
    /// Behind a line that mentions the user.
    pub highlight: &'static str,
    /// A line restored from the log, or one that was deleted.
    pub faded: &'static str,
    /// A link that can be clicked.
    pub link: &'static str,
    /// Behind what a search found, and behind the one it is at.
    pub found: &'static str,
    pub found_here: &'static str,
}

const LIGHT: Palette = Palette {
    dark: false,
    time: "#6b6b6b",
    notice: "#555555",
    warning: "#8a6116",
    error: "#b02525",
    event: "#7a3fa8",
    badge: "#2e7d32",
    emote: "#8b3fb5",
    highlight: "#fff3bf",
    faded: "#949494",
    link: "#1f5fbf",
    found: "#ffe08a",
    found_here: "#ff9f43",
};

const DARK: Palette = Palette {
    dark: true,
    time: "#808080",
    notice: "#a0a0a0",
    warning: "#d4a017",
    error: "#e05252",
    event: "#b06fd0",
    badge: "#5fa85f",
    emote: "#c586e0",
    highlight: "#5a4a00",
    faded: "#7a7a7a",
    link: "#6ca8ff",
    found: "#5c4a12",
    found_here: "#a85a00",
};

/// Tells GTK which way round to paint itself, and says which colours the
/// chat should then use.
///
/// `System` leaves GTK alone: it follows the desktop, and we ask it
/// afterwards what it decided rather than guessing.
pub fn apply(theme: Theme) -> &'static Palette {
    light_switches();
    let settings = gtk::Settings::default();
    let dark = match theme {
        Theme::Light => {
            set_dark(settings.as_ref(), false);
            false
        }
        Theme::Dark => {
            set_dark(settings.as_ref(), true);
            true
        }
        Theme::System => settings.as_ref().is_some_and(follows_dark_desktop),
    };
    flat_buttons_on_dark(dark);
    if dark { &DARK } else { &LIGHT }
}

/// Makes a flat button visibly light up under the pointer on a dark
/// background. GTK's dark theme raises it from grey 53 to 55, where the
/// buttons under the channel list hardly show they can be clicked; the
/// light theme goes from 246 to 218, about what this gives in the dark.
/// Only while dark, so the light theme stays GTK's own.
fn flat_buttons_on_dark(dark: bool) {
    thread_local! {
        static PROVIDER: std::cell::RefCell<Option<gtk::CssProvider>> =
            const { std::cell::RefCell::new(None) };
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    PROVIDER.with(|provider| {
        let mut provider = provider.borrow_mut();
        match (dark, provider.as_ref()) {
            (true, None) => {
                let css = gtk::CssProvider::new();
                css.load_from_data(
                    "button.flat:hover { background-image: none; \
                         background-color: alpha(currentColor, 0.14); }\n\
                     button.flat:active, button.flat:checked { background-image: none; \
                         background-color: alpha(currentColor, 0.22); }",
                );
                gtk::style_context_add_provider_for_display(
                    &display,
                    &css,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
                *provider = Some(css);
            }
            (false, Some(css)) => {
                gtk::style_context_remove_provider_for_display(&display, css);
                *provider = None;
            }
            _ => {}
        }
    });
}

/// The colours for text on a background of one's choosing: the dark set
/// on a dark one, so names and links stay readable.
pub fn for_background(background: Rgb) -> &'static Palette {
    if luminance(background) < 0.5 {
        &DARK
    } else {
        &LIGHT
    }
}

/// Gives switches a light knob in both themes. GTK's own dark theme
/// draws it dark grey, on the dark grey of a switch that is off and the
/// deep blue of one that is on, where it can hardly be told which side it
/// is on (seen 2026-10-19). Once per process; it holds for every window.
fn light_switches() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if DONE.with(|done| done.replace(true)) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let css = gtk::CssProvider::new();
    css.load_from_data(
        "switch > slider { background-image: none; background-color: #f6f5f4; }\n\
         switch:disabled > slider { background-color: #b8b8b8; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn set_dark(settings: Option<&gtk::Settings>, dark: bool) {
    let Some(settings) = settings else {
        return;
    };
    // GTK 4.20 replaced the old switch with a colour scheme and warns
    // whenever the old one is set. The new one is set by name, so the
    // same build still runs on an older GTK, which keeps the old switch.
    let nick = if dark { "dark" } else { "light" };
    let scheme = settings
        .find_property("gtk-interface-color-scheme")
        .and_then(|spec| glib::EnumClass::with_type(spec.value_type()))
        .and_then(|class| class.to_value_by_nick(nick));
    match scheme {
        Some(value) => settings.set_property_from_value("gtk-interface-color-scheme", &value),
        None => settings.set_gtk_application_prefer_dark_theme(dark),
    }
}

/// Whether the desktop's own choice is a dark one. GTK records it either
/// as the preference itself or in the name of the theme it picked.
fn follows_dark_desktop(settings: &gtk::Settings) -> bool {
    settings.is_gtk_application_prefer_dark_theme()
        || settings
            .gtk_theme_name()
            .is_some_and(|name| name.to_lowercase().contains("dark"))
}

/// Moves a colour far enough from the background to be read against it: up
/// towards white on a dark background, down towards black on a light one.
/// A viewer's chosen colour keeps its hue either way.
pub fn readable(rgb: Rgb, dark: bool) -> Rgb {
    const MIN_ON_DARK: f32 = 0.4;
    const MAX_ON_LIGHT: f32 = 0.55;
    let lum = luminance(rgb);
    if dark {
        if lum >= MIN_ON_DARK {
            return rgb;
        }
        // Blending with white raises luminance linearly, so this is exact.
        let t = (MIN_ON_DARK - lum) / (1.0 - lum);
        blend(rgb, 255.0, t)
    } else {
        if lum <= MAX_ON_LIGHT {
            return rgb;
        }
        let t = (lum - MAX_ON_LIGHT) / lum;
        blend(rgb, 0.0, t)
    }
}

fn luminance(rgb: Rgb) -> f32 {
    0.2126 * f32::from(rgb.r) / 255.0
        + 0.7152 * f32::from(rgb.g) / 255.0
        + 0.0722 * f32::from(rgb.b) / 255.0
}

fn blend(rgb: Rgb, towards: f32, t: f32) -> Rgb {
    let mix = |c: u8| {
        let c = f32::from(c);
        (c + (towards - c) * t).round().clamp(0.0, 255.0) as u8
    };
    Rgb {
        r: mix(rgb.r),
        g: mix(rgb.g),
        b: mix(rgb.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dark_name_is_lifted_only_on_a_dark_background() {
        let navy = Rgb { r: 0, g: 0, b: 90 };
        assert!(luminance(readable(navy, true)) >= 0.39);
        // On white it is already readable, so it is left as it is.
        assert_eq!(readable(navy, false), navy);
    }

    #[test]
    fn a_pale_name_is_darkened_only_on_a_light_background() {
        let pale = Rgb {
            r: 255,
            g: 255,
            b: 200,
        };
        assert!(luminance(readable(pale, false)) <= 0.56);
        assert_eq!(readable(pale, true), pale);
    }

    #[test]
    fn a_name_at_its_brightest_keeps_its_hue_when_lifted() {
        let blue = Rgb { r: 0, g: 0, b: 255 };
        let fixed = readable(blue, true);
        // Blue is already at its brightest, so the others come up instead.
        assert!(fixed.r > 0 && fixed.g > 0, "{fixed:?}");
        assert!(
            fixed.b >= fixed.r && fixed.b >= fixed.g,
            "{fixed:?} lost its hue"
        );
        // A colour that is already light is the viewer's choice; leave it.
        let light = Rgb {
            r: 0,
            g: 255,
            b: 127,
        };
        assert_eq!(readable(light, true), light);
    }

    #[test]
    fn hue_survives_both_ways() {
        let red = Rgb { r: 200, g: 0, b: 0 };
        let lifted = readable(red, true);
        assert!(lifted.r > lifted.g && lifted.r > lifted.b);
        let sunny = Rgb {
            r: 255,
            g: 240,
            b: 0,
        };
        let darkened = readable(sunny, false);
        assert!(darkened.r > darkened.b && darkened.g > darkened.b);
    }
}
