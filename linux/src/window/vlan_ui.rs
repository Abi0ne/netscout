//! "Abilita VLAN scan" in the scan panel (shown only when the scan's
//! interface can carry VLANs): the VLANs heard on the cable, the tag, and how
//! the computer gets an address in the VLAN. On two rows (short window) the
//! options open from a button beside the target.

use std::rc::Rc;

use adw::prelude::*;

use super::Window;
use crate::vlan::{Addressing, LISTEN_SECONDS};

/// An entry of the VLAN menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Nothing chosen yet (the menu's first line, a hint).
    Nothing,
    Tag(String),
    /// "Altro…": the tag is written by hand.
    Other,
}

pub struct VlanWidgets {
    /// Every option, moved between the panel and the compact popover.
    container: gtk::Box,
    enable: gtk::CheckButton,
    options: gtk::Box,
    choice: gtk::DropDown,
    choice_model: gtk::StringList,
    refresh: gtk::Button,
    spinner: adw::Spinner,
    tag_entry: gtk::Entry,
    addressing: adw::ToggleGroup,
    address_entry: gtk::Entry,
    status_icon: gtk::Image,
    status: gtk::Label,
    deactivate: gtk::Button,
    /// Where the options sit in the full panel.
    pub full_slot: gtk::Box,
    popover_slot: gtk::Box,
    /// The options' button on two rows.
    pub compact_button: gtk::MenuButton,
}

impl VlanWidgets {
    pub fn set_sensitive(&self, sensitive: bool) {
        self.container.set_sensitive(sensitive);
    }
}

pub fn build() -> VlanWidgets {
    let enable = gtk::CheckButton::with_label("Abilita VLAN scan");

    let choice_model = gtk::StringList::new(&[]);
    let choice = gtk::DropDown::new(Some(choice_model.clone()), None::<gtk::Expression>);
    choice.set_hexpand(true);
    choice.set_tooltip_text(Some(
        "Le VLAN sentite sul cavo, con la rete e i dispositivi visti in ciascuna; Altro… per scrivere il tag",
    ));
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.add_css_class("flat");
    refresh.set_tooltip_text(Some(&format!(
        "Ascolta di nuovo il cavo per {LISTEN_SECONDS} secondi"
    )));
    let spinner = adw::Spinner::new();
    spinner.set_visible(false);
    let menu_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    menu_row.append(&choice);
    menu_row.append(&spinner);
    menu_row.append(&refresh);

    let tag_entry = gtk::Entry::builder()
        .placeholder_text("Tag (1–4094)")
        .tooltip_text("ID della VLAN (1–4094)")
        .input_purpose(gtk::InputPurpose::Digits)
        .build();
    let addressing = adw::ToggleGroup::builder().homogeneous(true).build();
    addressing.add(adw::Toggle::builder().label("DHCP").name("dhcp").build());
    addressing.add(adw::Toggle::builder().label("Fisso").name("manual").build());
    addressing.set_active_name(Some("dhcp"));
    addressing.set_tooltip_text(Some(
        "Come ottenere un indirizzo nella VLAN: dal suo DHCP o uno fisso scritto qui",
    ));
    let address_entry = gtk::Entry::builder()
        .placeholder_text("172.20.20.250/24")
        .tooltip_text("Un indirizzo libero nella VLAN, con il prefisso della sua rete")
        .build();

    let status_icon = gtk::Image::from_icon_name("object-select-symbolic");
    status_icon.add_css_class("success");
    status_icon.set_valign(gtk::Align::Start);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status.set_hexpand(true);
    status.set_selectable(true);
    status.add_css_class("caption");
    let deactivate = gtk::Button::with_label("Disattiva");
    deactivate.add_css_class("flat");
    deactivate.add_css_class("small-button");
    deactivate.set_valign(gtk::Align::Start);
    deactivate.set_tooltip_text(Some("Rimuovi l'interfaccia VLAN dal computer"));
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    status_row.append(&status_icon);
    status_row.append(&status);
    status_row.append(&deactivate);

    let options = gtk::Box::new(gtk::Orientation::Vertical, 6);
    options.append(&menu_row);
    options.append(&tag_entry);
    options.append(&addressing);
    options.append(&address_entry);
    options.append(&status_row);

    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.append(&enable);
    container.append(&options);

    let full_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    full_slot.append(&container);
    let popover_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    popover_slot.set_width_request(280);
    popover_slot.set_margin_top(8);
    popover_slot.set_margin_bottom(8);
    popover_slot.set_margin_start(8);
    popover_slot.set_margin_end(8);
    let popover = gtk::Popover::new();
    popover.set_child(Some(&popover_slot));
    popover.set_position(gtk::PositionType::Right);
    let compact_button = gtk::MenuButton::builder()
        .icon_name("netscout-tag-symbolic")
        .popover(&popover)
        .visible(false)
        .build();
    compact_button.add_css_class("flat");

    VlanWidgets {
        container,
        enable,
        options,
        choice,
        choice_model,
        refresh,
        spinner,
        tag_entry,
        addressing,
        address_entry,
        status_icon,
        status,
        deactivate,
        full_slot,
        popover_slot,
        compact_button,
    }
}

impl Window {
    pub(super) fn connect_vlan(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.vlan.connect_changed(move || {
            if let Some(this) = weak.upgrade() {
                this.update_vlan_ui();
                this.update_scan_ui();
            }
        });
        let w = &self.w.vlan;
        let weak = Rc::downgrade(self);
        type Handler = Box<dyn Fn(&Rc<Window>)>;
        let guarded = move |f: Handler| {
            let weak = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    if !this.syncing_vlan.get() {
                        f(&this);
                    }
                }
            }
        };
        let f = guarded(Box::new(|this| {
            let on = this.w.vlan.enable.is_active();
            this.vlan.set_enabled(on);
        }));
        w.enable.connect_toggled(move |_| f());
        let f = guarded(Box::new(|this| {
            let i = this.w.vlan.choice.selected() as usize;
            let choice = this.vlan_choices.borrow().get(i).cloned();
            match choice {
                Some(Choice::Tag(tag)) => this.vlan.choose(Some(tag)),
                Some(Choice::Other) => this.vlan.choose(None),
                Some(Choice::Nothing) => this.vlan.choose(Some(String::new())),
                None => {}
            }
        }));
        w.choice.connect_selected_notify(move |_| f());
        let f = guarded(Box::new(|this| this.vlan.discover()));
        w.refresh.connect_clicked(move |_| f());
        let f = guarded(Box::new(|this| {
            let text = this.w.vlan.tag_entry.text().trim().to_string();
            this.vlan.set_tag(&text);
        }));
        w.tag_entry.connect_changed(move |_| f());
        let f = guarded(Box::new(|this| {
            let manual = this.w.vlan.addressing.active_name().as_deref() == Some("manual");
            this.vlan.set_addressing(if manual {
                Addressing::Manual
            } else {
                Addressing::Dhcp
            });
        }));
        w.addressing.connect_active_name_notify(move |_| f());
        let f = guarded(Box::new(|this| {
            let text = this.w.vlan.address_entry.text().trim().to_string();
            this.vlan.set_address(&text);
        }));
        w.address_entry.connect_changed(move |_| f());
        let f = guarded(Box::new(|this| this.vlan.deactivate()));
        w.deactivate.connect_clicked(move |_| f());
        // Enter in the fields starts the scan, like the target's.
        for entry in [&w.tag_entry, &w.address_entry] {
            let weak = Rc::downgrade(self);
            entry.connect_activate(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.start_scan();
                }
            });
        }
    }

    /// Bring the options up to date with the VLAN's state.
    pub(super) fn update_vlan_ui(&self) {
        let v = &self.vlan;
        let w = &self.w.vlan;
        self.syncing_vlan.set(true);

        let available = v.is_available();
        let compact = self.compact.get();
        w.full_slot.set_visible(available && !compact);
        w.compact_button.set_visible(available && compact);
        let slot = if compact { &w.popover_slot } else { &w.full_slot };
        super::move_to(&w.container, slot);
        if !w.compact_button.is_visible() {
            w.compact_button.popdown();
        }
        let parent = v.parent().unwrap_or_else(|| "Questa porta".into());
        w.enable.set_active(v.enabled());
        w.enable.set_tooltip_text(Some(&format!(
            "{parent} può ricevere VLAN taggate: scansiona anche una di quelle che arrivano sullo stesso cavo"
        )));
        w.options.set_visible(v.enabled());

        // The menu: a hint while nothing is chosen, the tag chosen earlier
        // if not heard this time, the VLANs heard, "Altro…".
        let tag = v.tag();
        let typing = v.typing_tag();
        let mut entries: Vec<(Choice, String)> = Vec::new();
        if tag.is_empty() && !typing {
            entries.push((
                Choice::Nothing,
                if v.is_discovering() {
                    "Ricerca delle VLAN…".into()
                } else {
                    "Scegli la VLAN".into()
                },
            ));
        } else if !typing && v.chosen().is_none() {
            entries.push((Choice::Tag(tag.clone()), format!("VLAN {tag}")));
        }
        for found in v.discovered() {
            entries.push((Choice::Tag(found.tag.to_string()), found.label()));
        }
        entries.push((Choice::Other, "Altro…".into()));
        let labels: Vec<&str> = entries.iter().map(|(_, l)| l.as_str()).collect();
        let shown: Vec<String> = (0..w.choice_model.n_items())
            .filter_map(|i| w.choice_model.string(i).map(|s| s.to_string()))
            .collect();
        if shown != labels {
            w.choice_model
                .splice(0, w.choice_model.n_items(), &labels);
        }
        let selected = entries
            .iter()
            .position(|(c, _)| match c {
                Choice::Other => typing,
                Choice::Tag(t) => !typing && *t == tag,
                Choice::Nothing => !typing && tag.is_empty(),
            })
            .unwrap_or(0);
        if w.choice.selected() as usize != selected {
            w.choice.set_selected(selected as u32);
        }
        *self.vlan_choices.borrow_mut() = entries.into_iter().map(|(c, _)| c).collect();

        w.spinner.set_visible(v.is_discovering());
        w.spinner.set_tooltip_text(Some(&format!(
            "In ascolto su {} per {LISTEN_SECONDS} secondi",
            v.parent().unwrap_or_default()
        )));
        w.refresh.set_visible(!v.is_discovering());
        w.tag_entry.set_visible(typing);
        if typing && w.tag_entry.text().trim() != tag {
            w.tag_entry.set_text(&tag);
        }
        let manual = v.addressing() == Addressing::Manual;
        w.addressing
            .set_active_name(Some(if manual { "manual" } else { "dhcp" }));
        w.address_entry.set_visible(manual);
        w.address_entry.set_placeholder_text(Some(
            &v.suggested_address()
                .unwrap_or_else(|| "172.20.20.250/24".into()),
        ));
        if w.address_entry.text().trim() != v.address() {
            w.address_entry.set_text(&v.address());
        }

        let (text, warning) = match v.active() {
            Some(a) => (
                format!("{} · {} · rete {}", a.interface, a.ipv4, a.cidr),
                false,
            ),
            None if v.is_preparing() => (
                if manual {
                    "Creazione dell'interfaccia…".to_string()
                } else {
                    "Creazione dell'interfaccia e richiesta al DHCP…".to_string()
                },
                false,
            ),
            None if v.is_discovering() => {
                let n = v.discovered().len();
                (
                    format!(
                        "In ascolto su {} per {LISTEN_SECONDS} secondi: {} finora.",
                        v.parent().unwrap_or_default(),
                        if n == 1 {
                            "1 VLAN trovata".to_string()
                        } else {
                            format!("{n} VLAN trovate")
                        }
                    ),
                    false,
                )
            }
            None => match v.discovery_problem().or_else(|| v.problem()) {
                Some(problem) => (problem, true),
                None => (
                    "NetScout crea l'interfaccia della VLAN all'avvio della scansione e la rimuove quando la disattivi o chiudi l'app. La password di amministratore si chiede una volta sola.".into(),
                    false,
                ),
            },
        };
        w.status.set_text(&text);
        if warning {
            w.status.remove_css_class("dim-label");
            w.status.add_css_class("warning");
        } else {
            w.status.remove_css_class("warning");
            w.status.add_css_class("dim-label");
        }
        let active = v.active().is_some();
        w.status_icon.set_visible(active);
        w.deactivate.set_visible(active);

        // The compact button: filled while a VLAN is scanned too, orange
        // when something is wrong.
        let b = &w.compact_button;
        b.set_tooltip_text(Some(&if v.is_on() {
            format!("Scansiona anche la VLAN {}", v.tag())
        } else {
            "Scansiona anche una VLAN taggata".into()
        }));
        for (class, on) in [
            ("accent", v.is_on() && v.problem().is_none()),
            ("warning", v.problem().is_some()),
        ] {
            if on {
                b.add_css_class(class);
            } else {
                b.remove_css_class(class);
            }
        }
        self.syncing_vlan.set(false);
    }
}
