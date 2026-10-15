//! NetScout for Linux: a GTK4 / libadwaita UI over `netscout-core`.
//!
//! `netscout --scan [--deep]` starts scanning the default network right away
//! (handy for testing); `NETSCOUT_DEBUG=1` logs diagnostics on stderr.
//! `netscout --agent …` is the root helper for tagged VLANs and the linked
//! switch, started by the app itself through pkexec (see `agent`).

mod agent;
mod config;
mod detail;
mod dhcp;
mod help;
mod labels;
mod segment;
mod settings;
mod table;
mod terminal;
mod vlan;
mod window;

use adw::prelude::*;
use gtk::{gdk, glib};

pub const APP_ID: &str = "io.github.abi0ne.NetScout";

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--agent") {
        return glib::ExitCode::from(agent::main(&args[2..]) as u8);
    }
    let scan_now = args.iter().any(|a| a == "--scan");
    let deep = args.iter().any(|a| a == "--deep");
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|app| {
        load_css();
        // Selectable values (device card, switch) are not selected whole
        // when a popover or the card gives them the focus.
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_label_select_on_focus(false);
        }
        let quit = gtk::gio::SimpleAction::new("quit", None);
        // Closing the windows (not `quit`) lets a window ask about unsaved notes.
        let a = app.clone();
        quit.connect_activate(move |_, _| {
            for w in a.windows() {
                w.close();
            }
        });
        app.add_action(&quit);
        app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
    });
    app.connect_activate(move |app| {
        // A second launch just raises the existing window.
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        let win = window::Window::new(app);
        win.window.present();
        if deep {
            win.set_deep_profile();
        }
        if scan_now {
            win.start_scan();
        }
        // The window is owned by GTK; keep the state alive with it.
        let keep = win.clone();
        win.window.connect_destroy(move |_| {
            // The VLAN helper would notice NetScout gone anyway; this is sooner.
            keep.shutdown();
        });
    });
    app.set_accels_for_action("window.close", &["<Ctrl>w"]);
    // GTK would reject our own flags (`--scan`); they are parsed above.
    app.run_with_args(&std::env::args().take(1).collect::<Vec<_>>())
}

/// The app's own symbolic icons (the VLAN tag), where the icon theme finds
/// them: written to the cache folder, as there is no resource bundle.
fn install_icons(display: &gdk::Display) {
    const ICONS: [(&str, &str); 1] = [(
        "netscout-tag-symbolic.svg",
        include_str!("../data/icons/symbolic/netscout-tag-symbolic.svg"),
    )];
    let dir = glib::user_cache_dir().join("netscout").join("icons");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, svg) in ICONS {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(svg) {
            let _ = std::fs::write(&path, svg);
        }
    }
    gtk::IconTheme::for_display(display).add_search_path(&dir);
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gdk::Display::default() {
        install_icons(&display);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Diagnostics on stderr, enabled by `NETSCOUT_DEBUG=1`.
pub fn debug_log(message: &str) {
    if std::env::var_os("NETSCOUT_DEBUG").is_some() {
        eprintln!("[netscout] {message}");
    }
}
