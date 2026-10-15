//! "Switch collegato": a header button on the scan tab (a network device
//! with a question mark) whose popover tells which switch, and which of its
//! ports, the computer's cable ends in, from the switch's LLDP or CDP
//! announcement (see `vlan::LinkedSwitch`).

use std::rc::Rc;

use adw::prelude::*;
use netscout_core::LinkNeighbor;

use super::Window;
use crate::labels;
use crate::vlan::{SwitchState, SWITCH_SECONDS};

pub struct SwitchWidgets {
    pub button: gtk::MenuButton,
    icon: gtk::Overlay,
    popover: gtk::Popover,
}

pub fn build() -> SwitchWidgets {
    let image = gtk::Image::from_icon_name("network-wired-symbolic");
    let mark = gtk::Label::new(Some("?"));
    mark.add_css_class("switch-mark");
    mark.set_halign(gtk::Align::End);
    mark.set_valign(gtk::Align::Start);
    mark.set_can_target(false);
    let icon = gtk::Overlay::new();
    icon.set_child(Some(&image));
    icon.add_overlay(&mark);
    let popover = gtk::Popover::new();
    popover.set_child(Some(&popover_content()));
    let button = gtk::MenuButton::builder()
        .child(&icon)
        .popover(&popover)
        .tooltip_text("Switch collegato: a quale switch e porta è collegato il computer (LLDP/CDP)")
        .build();
    SwitchWidgets {
        button,
        icon,
        popover,
    }
}

/// A fresh box for the popover's content (replaced whole on each change,
/// so no focused widget is pulled out from under GTK).
fn popover_content() -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_width_request(360);
    content.set_margin_top(10);
    content.set_margin_bottom(10);
    content.set_margin_start(10);
    content.set_margin_end(10);
    content
}

impl Window {
    pub(super) fn connect_switch(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.linked_switch.connect_changed(move || {
            if let Some(this) = weak.upgrade() {
                this.update_switch_ui();
            }
        });
        let weak = Rc::downgrade(self);
        self.w.switch.popover.connect_show(move |_| {
            let Some(this) = weak.upgrade() else { return };
            this.switch_reopen.set(false);
            // Listen when there is no answer yet for the scan's port.
            if let Some(port) = this.switch_port() {
                this.linked_switch.identify_if_needed(&port);
            }
            this.update_switch_ui();
        });
        // The password prompt takes the focus and closes the popover: it
        // opens again with the answer.
        let weak = Rc::downgrade(self);
        self.w.switch.popover.connect_closed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.switch_reopen.set(this.linked_switch.is_listening());
            }
        });
    }

    pub(super) fn update_switch_ui(self: &Rc<Self>) {
        let s = &self.w.switch;
        if self.linked_switch.is_listening() {
            s.icon.add_css_class("listening");
        } else {
            s.icon.remove_css_class("listening");
            if self.switch_reopen.replace(false) && s.button.is_visible() {
                s.button.popup();
            }
        }
        let content = &popover_content();
        s.popover.set_child(Some(content));
        let Some(port) = self.switch_port() else {
            content.append(&message(
                "network-wired-disconnected-symbolic",
                "dim-label",
                "Il computer non è collegato via cavo",
                "Gli switch annunciano chi sono solo sul cavo: in Wi‑Fi l'access point non fa passare gli annunci. Collega il computer a una porta di rete.",
            ));
            return;
        };
        match self.linked_switch.state() {
            SwitchState::Idle => content.append(&self.retry("Identifica lo switch", &port)),
            SwitchState::Listening { port } => content.append(&self.listening(&port)),
            SwitchState::Found { neighbor, port, at } => {
                self.neighbor_details(content, &neighbor, &port, at)
            }
            SwitchState::Silent { port } => {
                content.append(&message(
                    "dialog-warning-symbolic",
                    "warning",
                    "Nessun annuncio dallo switch",
                    &format!("In {SWITCH_SECONDS} secondi su {port} non è arrivato nessun annuncio LLDP o CDP: lo switch non è gestito oppure ha questi annunci disattivati."),
                ));
                content.append(&self.retry("Ascolta di nuovo", &port));
            }
            SwitchState::Failed(problem) => {
                content.append(&message(
                    "dialog-warning-symbolic",
                    "warning",
                    "Switch non identificato",
                    &problem,
                ));
                content.append(&self.retry("Riprova", &port));
            }
        }
    }

    fn retry(self: &Rc<Self>, title: &str, port: &str) -> gtk::Widget {
        let b = gtk::Button::with_label(title);
        b.add_css_class("suggested-action");
        b.set_halign(gtk::Align::End);
        let (weak, port) = (Rc::downgrade(self), port.to_string());
        b.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.linked_switch.identify(&port);
            }
        });
        b.upcast()
    }

    fn listening(self: &Rc<Self>, port: &str) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let spinner = adw::Spinner::new();
        spinner.set_valign(gtk::Align::Start);
        spinner.set_margin_top(2);
        row.append(&spinner);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        let title = label("In ascolto dello switch…");
        title.add_css_class("heading");
        text.append(&title);
        let body = label(&format!("Su {port}. Gli switch si annunciano ogni 30 secondi circa (Cisco ogni 60): l'attesa dura al massimo {SWITCH_SECONDS} secondi."));
        body.add_css_class("dim-label");
        text.append(&body);
        let cancel = gtk::Button::with_label("Annulla");
        cancel.add_css_class("flat");
        cancel.set_halign(gtk::Align::Start);
        let weak = Rc::downgrade(self);
        cancel.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.linked_switch.cancel();
            }
        });
        text.append(&cancel);
        row.append(&text);
        row.upcast()
    }

    /// What the switch announced.
    fn neighbor_details(self: &Rc<Self>, content: &gtk::Box, n: &LinkNeighbor, port: &str, at: i64) {
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let icon = gtk::Image::from_icon_name(neighbor_icon(n));
        icon.set_pixel_size(32);
        icon.add_css_class("accent");
        icon.set_valign(gtk::Align::Start);
        head.append(&icon);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
        text.set_hexpand(true);
        let name = selectable(
            n.system_name
                .as_deref()
                .or(n.chassis_id.as_deref())
                .unwrap_or("Dispositivo senza nome"),
        );
        name.add_css_class("title-3");
        text.append(&name);
        if let Some(description) = n.model.as_ref().or(n.description.as_ref()) {
            let d = selectable(description);
            d.add_css_class("dim-label");
            text.append(&d);
        }
        head.append(&text);
        content.append(&head);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let grid = gtk::Grid::new();
        grid.set_column_spacing(14);
        grid.set_row_spacing(7);
        let mut row = 0;
        let mut add = |title: &str, value: gtk::Widget| {
            let t = gtk::Label::new(Some(title));
            t.set_xalign(1.0);
            t.set_yalign(0.0);
            t.add_css_class("dim-label");
            grid.attach(&t, 0, row, 1, 1);
            grid.attach(&value, 1, row, 1, 1);
            row += 1;
        };
        if let Some(p) = &n.port {
            let v = selectable(p);
            v.add_css_class("heading");
            v.add_css_class("numeric");
            add("Porta dello switch", v.upcast());
        }
        if let Some(d) = n.port_description.as_ref().filter(|d| Some(*d) != n.port.as_ref()) {
            add("Descrizione porta", selectable(d).upcast());
        }
        if let Some(ip) = &n.management_ip {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let v = selectable(ip);
            v.add_css_class("numeric");
            v.set_hexpand(false);
            b.append(&v);
            let open = gtk::Button::from_icon_name("adw-external-link-symbolic");
            open.add_css_class("flat");
            open.add_css_class("small-button");
            open.set_tooltip_text(Some("Apri la pagina dello switch nel browser"));
            let (weak, url) = (Rc::downgrade(self), format!("http://{ip}"));
            open.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.open_url(&url);
                }
            });
            b.append(&open);
            add("Gestione", b.upcast());
        }
        if let Some(id) = &n.chassis_id {
            let v = selectable(id);
            v.add_css_class("monospace");
            add("Identificativo", v.upcast());
        }
        if !n.capabilities.is_empty() {
            add("Tipo", selectable(&n.capabilities.join(", ")).upcast());
        }
        content.append(&grid);
        if let (Some(description), Some(_)) = (&n.description, &n.model) {
            let d = selectable(description);
            d.add_css_class("caption");
            d.add_css_class("dim-label");
            content.append(&d);
        }
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let when = gtk::Label::new(Some(&format!(
            "{} · {port} · {}",
            n.protocol,
            labels::format_time(at, false)
        )));
        when.add_css_class("caption");
        when.add_css_class("dim-label");
        when.set_hexpand(true);
        when.set_xalign(0.0);
        foot.append(&when);
        let again = gtk::Button::from_icon_name("view-refresh-symbolic");
        again.add_css_class("flat");
        again.set_tooltip_text(Some("Ascolta di nuovo l'annuncio"));
        let (weak, p) = (Rc::downgrade(self), port.to_string());
        again.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.linked_switch.identify(&p);
            }
        });
        foot.append(&again);
        let found = n
            .management_ip
            .clone()
            .filter(|ip| self.live_hosts().iter().any(|h| &h.ip == ip));
        if let Some(ip) = found {
            let show = gtk::Button::with_label("Mostra nella tabella");
            let weak = Rc::downgrade(self);
            show.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.show_in_table(&ip);
                }
            });
            foot.append(&show);
        }
        content.append(&foot);
    }

    /// Select the device with `ip` among all those found, with its card.
    fn show_in_table(self: &Rc<Self>, ip: &str) {
        self.w.switch.popover.popdown();
        {
            let mut f = self.filter_state.borrow_mut();
            f.device_type = None;
            f.segment = None;
        }
        self.w.search.set_text("");
        self.filter_state.borrow_mut().query.clear();
        self.refilter();
        self.rebuild_types();
        self.select_id(ip);
        self.w.detail_split.set_show_sidebar(true);
        self.show_detail();
    }
}

fn neighbor_icon(n: &LinkNeighbor) -> &'static str {
    let has = |c: &str| n.capabilities.iter().any(|x| x == c);
    if has("Switch") {
        "network-wired-symbolic"
    } else if has("Router") || has("Access point") {
        "network-wireless-symbolic"
    } else if has("Telefono") {
        "call-start-symbolic"
    } else {
        "network-workgroup-symbolic"
    }
}

fn message(icon: &str, tint: &str, title: &str, text: &str) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(24);
    image.add_css_class(tint);
    image.set_valign(gtk::Align::Start);
    row.append(&image);
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.set_hexpand(true);
    let t = label(title);
    t.add_css_class("heading");
    b.append(&t);
    let body = label(text);
    body.add_css_class("dim-label");
    b.append(&body);
    row.append(&b);
    row.upcast()
}

fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_max_width_chars(40);
    l
}

fn selectable(text: &str) -> gtk::Label {
    let l = label(text);
    l.set_selectable(true);
    l
}
