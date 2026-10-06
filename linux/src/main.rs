//! NetScout for Linux: a GTK4 / libadwaita UI over `netscout-core`.
//!
//! `netscout --scan` starts scanning the default network right away (handy
//! for testing); `NETSCOUT_DEBUG=1` logs diagnostics on stderr.

mod config;
mod detail;
mod help;
mod labels;
mod table;
mod terminal;
mod window;

use adw::prelude::*;
use gtk::{gdk, glib};

pub const APP_ID: &str = "io.github.abi0ne.NetScout";

fn main() -> glib::ExitCode {
    let scan_now = std::env::args().any(|a| a == "--scan");
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|app| {
        load_css();
        let quit = gtk::gio::SimpleAction::new("quit", None);
        let a = app.clone();
        quit.connect_activate(move |_, _| a.quit());
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
        if scan_now {
            win.start_scan();
        }
        // The window is owned by GTK; keep the state alive with it.
        let keep = win.clone();
        win.window.connect_destroy(move |_| {
            let _ = &keep;
        });
    });
    app.set_accels_for_action("window.close", &["<Ctrl>w"]);
    // GTK would reject our own flags (`--scan`); they are parsed above.
    app.run_with_args(&std::env::args().take(1).collect::<Vec<_>>())
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gdk::Display::default() {
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
