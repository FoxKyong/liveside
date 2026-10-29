//! Which Liveside this is, who makes it, and under what licence: GTK's
//! own About window, opened from the settings. The version is what a bug
//! report needs first.

use gtk::prelude::*;
use liveside_client::lang::Strings;

/// The launcher's icon, built in for a copy run from where it was built,
/// which has none installed.
const ICON: &[u8] = include_bytes!("../../../packaging/linux/net.jifox.liveside.Liveside.svg");
const ICON_NAME: &str = "net.jifox.liveside.Liveside";

pub fn open(parent: &impl IsA<gtk::Window>, strings: &'static Strings) {
    let about = gtk::AboutDialog::builder()
        .transient_for(parent)
        .modal(true)
        .program_name("Liveside")
        .version(env!("CARGO_PKG_VERSION"))
        .comments(strings.about_summary)
        .website("https://liveside.jifox.net")
        .website_label("liveside.jifox.net")
        .authors(["Radim Vančo (FoxKyong)"])
        .copyright("© 2026 Radim Vančo")
        .license_type(gtk::License::Gpl30)
        .build();
    let installed = gtk::IconTheme::for_display(&WidgetExt::display(&about)).has_icon(ICON_NAME);
    if installed {
        about.set_logo_icon_name(Some(ICON_NAME));
    } else if let Ok(texture) = gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_static(ICON))
    {
        about.set_logo(Some(&texture));
    }
    about.present();
}
