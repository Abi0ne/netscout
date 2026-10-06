//! Asks with which user (and password) to open an SSH/Telnet session, then
//! opens it in a terminal. The user name is remembered per host; the
//! password is never stored.

use std::rc::Rc;

use adw::prelude::*;
use netscout_core::Host;

use super::Window;
use crate::config;
use crate::terminal::{self, Remote};

impl Window {
    pub(super) fn connect_remote(self: &Rc<Self>, host: &Host, kind: Remote, port: u16) {
        if let Some(reason) = terminal::unavailable_reason(kind) {
            self.show_error(&reason);
            return;
        }
        if kind == Remote::Rdp {
            if let Err(e) = terminal::open_rdp(&host.ip, port) {
                self.show_error(&e);
            }
            return;
        }

        let user_key = format!("user.{}", host.ip);
        let dialog = adw::AlertDialog::new(
            Some(&format!("{} nel terminale", kind.label())),
            Some(&format!("{} · porta {port}", host.ip)),
        );
        let group = adw::PreferencesGroup::new();
        let user = adw::EntryRow::builder()
            .title("Utente")
            .text(config::get("terminal", &user_key).unwrap_or_default())
            .activates_default(true)
            .build();
        group.add(&user);
        let password = adw::PasswordEntryRow::builder()
            .title("Password")
            .activates_default(true)
            .build();
        let send_password = terminal::can_send_password();
        if send_password {
            group.add(&password);
        }
        let note = gtk::Label::new(Some(if send_password {
            "Lascia vuota la password per inserirla nel terminale (o per usare una chiave SSH). La password non viene salvata."
        } else {
            "La password si inserisce nel terminale. Per inserirla qui installa expect (sudo apt install expect)."
        }));
        note.set_wrap(true);
        note.set_xalign(0.0);
        note.add_css_class("caption");
        note.add_css_class("dim-label");
        let extra = gtk::Box::new(gtk::Orientation::Vertical, 12);
        extra.append(&group);
        extra.append(&note);
        dialog.set_extra_child(Some(&extra));
        dialog.add_responses(&[("cancel", "Annulla"), ("connect", "Connetti")]);
        dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("connect"));
        dialog.set_close_response("cancel");

        let weak = Rc::downgrade(self);
        let ip = host.ip.clone();
        let (user_row, password_row) = (user.clone(), password.clone());
        dialog.connect_response(None, move |_, response| {
            if response != "connect" {
                return;
            }
            let name = user_row.text().trim().to_string();
            let result = terminal::open(kind, &ip, port, &name, &password_row.text());
            match result {
                Ok(()) => config::set("terminal", &user_key, &name),
                Err(e) => {
                    if let Some(this) = weak.upgrade() {
                        this.show_error(&e);
                    }
                }
            }
        });
        dialog.present(Some(&self.window));
        if user.text().is_empty() || !send_password {
            user.grab_focus();
        } else {
            password.grab_focus();
        }
    }
}
