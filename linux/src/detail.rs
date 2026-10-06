//! The device card on the right: identity, addresses, latency and open ports,
//! with the actions on them (browser, terminal, deep scan, Wake-on-LAN).

use std::rc::Rc;

use adw::prelude::*;
use netscout_core::Host;

use crate::labels;
use crate::terminal::{self, Remote};

/// Sends a Wake-on-LAN packet; `Err` carries the message to show.
pub type WakeFn = Rc<dyn Fn(&Host) -> Result<(), String>>;
/// Opens a remote session to a host on a port.
pub type ConnectFn = Box<dyn Fn(&Host, Remote, u16)>;

/// What the card can ask the window to do.
pub struct Actions {
    /// Re-scan this IP with the deep profile.
    pub deep_scan: Box<dyn Fn(&str)>,
    /// Open a URL in the default browser.
    pub open_url: Box<dyn Fn(&str)>,
    /// Drop a device that is off from the table (by row id).
    pub forget: Box<dyn Fn(&str)>,
    pub wake: WakeFn,
    pub connect: ConnectFn,
}

/// Placeholder shown with nothing selected.
pub fn empty() -> gtk::Widget {
    adw::StatusPage::builder()
        .icon_name("network-workgroup-symbolic")
        .title("Nessun dispositivo selezionato")
        .description("Seleziona un dispositivo dalla lista.")
        .css_classes(["compact"])
        .build()
        .upcast()
}

pub fn build(host: &Host, offline: bool, deep_scanning: bool, actions: Actions) -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    content.append(&header(host, offline));
    content.append(&deep_scan_button(
        host,
        offline,
        deep_scanning,
        actions.deep_scan,
    ));
    if offline {
        content.append(&wake_section(host, actions.wake, actions.forget));
    }
    content.append(&info_grid(host, offline));
    content.append(&ports_section(
        host,
        offline,
        actions.open_url,
        actions.connect,
    ));

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_child(Some(&content));
    scroller.upcast()
}

fn header(host: &Host, offline: bool) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let icon = gtk::Image::from_icon_name(labels::device_icon(host.device_type));
    icon.set_pixel_size(32);
    icon.add_css_class("device-badge");
    icon.set_valign(gtk::Align::Center);
    row.append(&icon);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_valign(gtk::Align::Center);
    text.set_hexpand(true);
    let name = labels::display_name(host).unwrap_or_else(|| host.ip.clone());
    let title = selectable(&name);
    title.add_css_class("title-2");
    text.append(&title);
    let subtitle: Vec<&str> = std::iter::once(labels::device_label(host.device_type))
        .chain(host.vendor.as_deref())
        .collect();
    let sub = wrapped(&subtitle.join(" · "));
    sub.add_css_class("dim-label");
    text.append(&sub);
    row.append(&text);

    if offline {
        let badge = gtk::Label::new(Some("Spento"));
        badge.add_css_class("off-badge");
        badge.set_valign(gtk::Align::Center);
        row.append(&badge);
    }
    row.upcast()
}

fn deep_scan_button(
    host: &Host,
    offline: bool,
    running: bool,
    deep_scan: Box<dyn Fn(&str)>,
) -> gtk::Widget {
    let button = gtk::Button::new();
    button.set_halign(gtk::Align::Start);
    if running {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        b.append(&adw::Spinner::new());
        b.append(&gtk::Label::new(Some("Scansione in corso…")));
        button.set_child(Some(&b));
        button.set_sensitive(false);
    } else {
        let content = adw::ButtonContent::builder()
            .icon_name("edit-find-symbolic")
            .label(if offline {
                "Riscansiona"
            } else {
                "Scansione approfondita"
            })
            .build();
        button.set_child(Some(&content));
    }
    button.set_tooltip_text(Some(if offline {
        "Riscansiona questo indirizzo per vedere se il dispositivo si è acceso"
    } else {
        "Riscansiona questo dispositivo con il profilo approfondito"
    }));
    let ip = host.ip.clone();
    button.connect_clicked(move |_| deep_scan(&ip));
    button.upcast()
}

fn wake_section(host: &Host, wake: WakeFn, forget: Box<dyn Fn(&str)>) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let note = wrapped("Non trovato in questa scansione: è spento o non è più in rete.");
    note.add_css_class("dim-label");
    b.append(&note);
    b.append(&wake_button(host, false, wake));
    let forget_button = gtk::Button::with_label("Dimentica dispositivo spento");
    forget_button.set_halign(gtk::Align::Start);
    forget_button.add_css_class("flat");
    forget_button.set_tooltip_text(Some(
        "Toglie il dispositivo dall'elenco di questa scansione",
    ));
    let id = crate::table::offline_id(host);
    forget_button.connect_clicked(move |_| forget(&id));
    b.append(&forget_button);
    b.upcast()
}

/// Sends a Wake-on-LAN packet to a device that is off, and says what
/// happened. `compact` is the form for lists (short label and status).
pub fn wake_button(host: &Host, compact: bool, wake: WakeFn) -> gtk::Widget {
    let b = gtk::Box::new(
        if compact {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        },
        8,
    );
    let button = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("system-shutdown-symbolic")
                .label(if compact {
                    "Accendi"
                } else {
                    "Accendi (Wake-on-LAN)"
                })
                .build(),
        )
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .sensitive(host.mac.is_some())
        .build();
    if compact {
        button.add_css_class("flat");
    }
    button.set_tooltip_text(Some(&match &host.mac {
        None => "MAC sconosciuto: il Wake-on-LAN non è possibile.".to_string(),
        Some(_) if labels::has_private_mac(host) => {
            "MAC privato (casuale): questo dispositivo probabilmente non si accende con il Wake-on-LAN.".to_string()
        }
        Some(mac) => format!("Invia il pacchetto Wake-on-LAN a {mac}"),
    }));
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(!compact);
    status.add_css_class("caption");
    status.set_visible(false);
    let host = host.clone();
    let s = status.clone();
    button.connect_clicked(move |_| {
        s.set_visible(true);
        match wake(&host) {
            Ok(()) => {
                let now = labels::format_clock(gtk::glib::real_time() / 1000);
                s.remove_css_class("error");
                s.add_css_class("dim-label");
                s.set_text(&if compact {
                    format!("Inviato alle {now}")
                } else {
                    format!("Pacchetto inviato alle {now}. Se il dispositivo ha il Wake-on-LAN attivo si accende in qualche secondo: riscansionalo per verificare.")
                });
            }
            Err(message) => {
                s.remove_css_class("dim-label");
                s.add_css_class("error");
                s.set_text(&message);
            }
        }
    });
    b.append(&button);
    b.append(&status);
    b.upcast()
}

fn info_grid(host: &Host, offline: bool) -> gtk::Widget {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(16);
    grid.set_row_spacing(8);
    let mut rows: Vec<(&str, String, bool)> = vec![
        ("Indirizzo IP", host.ip.clone(), true),
        ("MAC", host.mac.clone().unwrap_or_else(|| "—".into()), true),
        (
            "Produttore",
            host.vendor.clone().unwrap_or_else(|| mac_note(host)),
            false,
        ),
        (
            "Nomi",
            if host.hostnames.is_empty() {
                "—".into()
            } else {
                host.hostnames.join("\n")
            },
            false,
        ),
    ];
    if offline {
        rows.push((
            "Visto l'ultima volta",
            labels::format_time(host.last_seen, true),
            false,
        ));
    } else {
        rows.push((
            "Latenza",
            host.rtt_ms
                .map(|r| format!("{r:.2} ms"))
                .unwrap_or_else(|| "Non risponde al ping".into()),
            false,
        ));
        rows.push(("Visto alle", labels::format_clock(host.last_seen), false));
    }
    for (i, (title, value, mono)) in rows.into_iter().enumerate() {
        let t = gtk::Label::new(Some(title));
        t.set_xalign(0.0);
        t.set_yalign(0.0);
        t.add_css_class("dim-label");
        let v = selectable(&value);
        if mono {
            v.add_css_class("monospace");
            v.set_wrap(false);
        }
        grid.attach(&t, 0, i as i32, 1, 1);
        grid.attach(&v, 1, i as i32, 1, 1);
    }
    grid.upcast()
}

/// Why a MAC has no vendor, when we can tell.
fn mac_note(host: &Host) -> String {
    match host.mac {
        None => "—".into(),
        Some(_) if labels::has_private_mac(host) => "MAC privato (casuale)".into(),
        Some(_) => "Non registrato".into(),
    }
}

fn ports_section(
    host: &Host,
    offline: bool,
    open_url: Box<dyn Fn(&str)>,
    connect: ConnectFn,
) -> gtk::Widget {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let title = gtk::Label::new(Some(if offline {
        "Porte aperte (all'ultimo rilevamento)"
    } else {
        "Porte aperte"
    }));
    title.set_xalign(0.0);
    title.add_css_class("heading");
    section.append(&title);

    if host.open_ports.is_empty() {
        let none = gtk::Label::new(Some("Nessuna tra quelle verificate."));
        none.set_xalign(0.0);
        none.add_css_class("dim-label");
        section.append(&none);
        return section.upcast();
    }

    let open_url = Rc::new(open_url);
    let connect = Rc::new(connect);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    for port in &host.open_ports {
        let row = adw::ActionRow::builder()
            .title(port.number.to_string())
            .subtitle(port.service.as_deref().unwrap_or("—"))
            .css_classes(["property"])
            .build();
        if let Some(version) = &port.version {
            row.set_tooltip_text(Some(version));
        }
        if let Some(url) = labels::web_url(&host.ip, port.number) {
            let open = link_button("Apri", &url);
            let open_url = open_url.clone();
            open.connect_clicked(move |_| open_url(&url));
            row.add_suffix(&open);
        }
        if let Some(kind) = Remote::for_port(port).filter(|_| !offline) {
            let button = link_button(kind.button_label(), "");
            match terminal::unavailable_reason(kind) {
                Some(reason) => {
                    button.set_sensitive(false);
                    button.set_tooltip_text(Some(&reason));
                }
                None => button.set_tooltip_text(Some(&kind.help())),
            }
            let (connect, host, number) = (connect.clone(), host.clone(), port.number);
            button.connect_clicked(move |_| connect(&host, kind, number));
            row.add_suffix(&button);
        }
        list.append(&row);
    }
    section.append(&list);
    section.upcast()
}

fn link_button(label: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_valign(gtk::Align::Center);
    b.add_css_class("flat");
    if !tooltip.is_empty() {
        b.set_tooltip_text(Some(tooltip));
    }
    b
}

fn selectable(text: &str) -> gtk::Label {
    let l = wrapped(text);
    l.set_selectable(true);
    l
}

fn wrapped(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l
}
