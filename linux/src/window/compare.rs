//! The differences between the finished scan and a saved profile.

use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use netscout_core::{diff_hosts, Host, HostChange, ProfileSummary, ScanDiff};

use super::Window;
use crate::{detail, labels, table};

impl Window {
    /// Compare the finished scan with profile `id` and show the result.
    pub(super) fn compare_with(self: &Rc<Self>, id: &str) {
        if !self.has_finished_scan() {
            return;
        }
        let Some(saved) = self.load_profile(id) else {
            return;
        };
        let Some(summary) = self.profiles.borrow().iter().find(|p| p.id == id).cloned() else {
            return;
        };
        let diff = diff_hosts(saved.hosts, saved.offline_hosts, self.live_hosts());
        let target_differs = saved.target != *self.scanned_target.borrow();
        let depth_differs = saved.scan_profile != self.scanned_profile.get();
        self.show_comparison(summary, diff, target_differs, depth_differs);
    }

    fn show_comparison(
        self: &Rc<Self>,
        profile: ProfileSummary,
        diff: ScanDiff,
        target_differs: bool,
        depth_differs: bool,
    ) {
        let came_back: Vec<&HostChange> = diff.changed.iter().filter(|c| c.came_back).collect();
        let moved: Vec<&HostChange> = diff
            .changed
            .iter()
            .filter(|c| !c.came_back && c.ip_changed)
            .collect();
        let other: Vec<&HostChange> = diff
            .changed
            .iter()
            .filter(|c| !c.came_back && !c.ip_changed)
            .collect();
        // Devices of the profile not found now: gone off, still off, or whose
        // address another device now uses.
        let offline: Vec<Host> = diff
            .removed
            .iter()
            .chain(&diff.still_offline)
            .cloned()
            .chain(diff.replaced.iter().map(|c| c.before.clone()))
            .collect();
        let has_differences = !diff.added.is_empty()
            || !diff.removed.is_empty()
            || !diff.still_offline.is_empty()
            || !diff.changed.is_empty()
            || !diff.replaced.is_empty();

        let dialog = adw::Dialog::builder()
            .title(format!("Confronto con «{}»", profile.name))
            .content_width(780)
            .content_height(640)
            .build();

        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
        body.set_margin_top(12);
        body.set_margin_bottom(24);
        body.set_margin_start(24);
        body.set_margin_end(24);
        body.append(&self.comparison_header(
            &profile,
            &diff,
            &came_back,
            &moved,
            &other,
            target_differs,
            depth_differs,
        ));

        let wake = self.detail_actions().wake;
        if has_differences {
            let group = |title: &str, rows: Vec<adw::ActionRow>| {
                if rows.is_empty() {
                    return;
                }
                let g = adw::PreferencesGroup::builder()
                    .title(format!("{title} ({})", rows.len()))
                    .build();
                for r in rows {
                    g.add(&r);
                }
                body.append(&g);
            };
            group(
                "Nuovi dispositivi",
                diff.added
                    .iter()
                    .map(|h| host_row(h, "list-add-symbolic", "success", &[]))
                    .collect(),
            );
            group(
                "Spenti o scomparsi",
                diff.removed
                    .iter()
                    .map(|h| offline_row(h, &[], wake.clone()))
                    .collect(),
            );
            group(
                "Ancora spenti",
                diff.still_offline
                    .iter()
                    .map(|h| offline_row(h, &[], wake.clone()))
                    .collect(),
            );
            group(
                "Di nuovo accesi",
                came_back
                    .iter()
                    .map(|c| change_row(c, "system-shutdown-symbolic", "success"))
                    .collect(),
            );
            group(
                "Stesso MAC, IP diverso",
                moved
                    .iter()
                    .map(|c| change_row(c, "view-dual-symbolic", "accent"))
                    .collect(),
            );
            group(
                "Stesso IP, MAC diverso",
                diff.replaced
                    .iter()
                    .map(|c| {
                        let before = format!(
                            "Prima a questo indirizzo c'era un altro dispositivo, ora non trovato: {}",
                            host_line(&c.before)
                        );
                        host_row(&c.after, "dialog-warning-symbolic", "warning", &[before])
                    })
                    .collect(),
            );
            group(
                "Altre modifiche",
                other
                    .iter()
                    .map(|c| change_row(c, "document-edit-symbolic", "warning"))
                    .collect(),
            );
        } else {
            body.append(
                &adw::StatusPage::builder()
                    .icon_name("object-select-symbolic")
                    .title("Nessuna differenza")
                    .description("La rete è uguale a quella salvata nel profilo.")
                    .css_classes(["compact"])
                    .build(),
            );
        }

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&body));

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        footer.set_margin_top(12);
        footer.set_margin_bottom(12);
        footer.set_margin_start(12);
        footer.set_margin_end(12);
        if !offline.is_empty() {
            let add = gtk::Button::with_label(&format!(
                "Aggiungi gli spenti alla scansione ({})",
                offline.len()
            ));
            add.set_tooltip_text(Some(
                "Mostra nella scansione i dispositivi non trovati, per accenderli con il Wake-on-LAN",
            ));
            let weak = Rc::downgrade(self);
            let devices = offline.clone();
            add.connect_clicked(move |b| {
                if let Some(this) = weak.upgrade() {
                    this.add_offline(&devices);
                    b.set_label("Spenti aggiunti alla scansione");
                    b.set_sensitive(false);
                }
            });
            footer.append(&add);
        }
        let update = gtk::Button::with_label("Aggiorna il profilo…");
        update.set_tooltip_text(Some(
            "Salva questa scansione nel profilo, compresi i dispositivi spenti",
        ));
        {
            let weak = Rc::downgrade(self);
            let dialog = dialog.clone();
            update.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.confirm_update_profile(&dialog, &profile, &offline);
                }
            });
        }
        footer.append(&update);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        let close = gtk::Button::with_label("Chiudi");
        close.add_css_class("suggested-action");
        {
            let dialog = dialog.clone();
            close.connect_clicked(move |_| {
                dialog.close();
            });
        }
        footer.append(&close);
        dialog.set_default_widget(Some(&close));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&scroller));
        toolbar.add_bottom_bar(&footer);
        toolbar.set_bottom_bar_style(adw::ToolbarStyle::Raised);
        dialog.set_child(Some(&toolbar));
        dialog.present(Some(&self.window));
    }

    #[allow(clippy::too_many_arguments)]
    fn comparison_header(
        &self,
        profile: &ProfileSummary,
        diff: &ScanDiff,
        came_back: &[&HostChange],
        moved: &[&HostChange],
        other: &[&HostChange],
        target_differs: bool,
        depth_differs: bool,
    ) -> gtk::Widget {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let sub = gtk::Label::new(Some(&format!(
            "Profilo salvato {} · {}",
            labels::format_time(profile.created_at, true),
            profile.target
        )));
        sub.set_xalign(0.0);
        sub.add_css_class("dim-label");
        b.append(&sub);

        let badges = gtk::FlowBox::new();
        badges.set_selection_mode(gtk::SelectionMode::None);
        badges.set_max_children_per_line(6);
        badges.set_column_spacing(8);
        badges.set_row_spacing(8);
        for (count, label, class) in [
            (diff.added.len(), "nuovi", "success"),
            (
                diff.removed.len() + diff.still_offline.len(),
                "spenti",
                "error",
            ),
            (moved.len(), "IP cambiato", "accent"),
            (diff.replaced.len(), "MAC cambiato", "warning"),
            (came_back.len() + other.len(), "modificati", "warning"),
            (diff.unchanged as usize, "invariati", ""),
        ] {
            let l = gtk::Label::new(Some(&format!("{count} {label}")));
            l.add_css_class("count-badge");
            l.add_css_class("numeric");
            if count > 0 && !class.is_empty() {
                l.add_css_class(class);
            } else {
                l.add_css_class("dim-label");
            }
            badges.insert(&l, -1);
        }
        b.append(&badges);

        if target_differs || depth_differs {
            let mut parts = Vec::new();
            if target_differs {
                parts.push(format!(
                    "la scansione ha una destinazione diversa ({} nel profilo)",
                    profile.target
                ));
            }
            if depth_differs {
                parts.push(format!(
                    "il profilo è stato salvato con la scansione {}: le porte non verificate in una delle due risultano aperte o chiuse",
                    labels::profile_label(profile.scan_profile).to_lowercase()
                ));
            }
            let caveat = gtk::Label::new(Some(&format!("Attenzione: {}.", parts.join("; "))));
            caveat.set_xalign(0.0);
            caveat.set_wrap(true);
            caveat.add_css_class("warning");
            b.append(&caveat);
        }
        b.upcast()
    }

    fn confirm_update_profile(
        self: &Rc<Self>,
        parent: &adw::Dialog,
        profile: &ProfileSummary,
        offline: &[Host],
    ) {
        let already: HashSet<String> = self.offline_hosts().iter().map(table::offline_id).collect();
        let off_count = already
            .union(&offline.iter().map(table::offline_id).collect())
            .count();
        let alert = adw::AlertDialog::new(
            Some(&format!("Aggiornare il profilo «{}»?", profile.name)),
            Some(&format!(
                "Il profilo verrà sostituito da questa scansione: {} dispositivi accesi e {off_count} spenti, che restano nel profilo per il Wake-on-LAN.",
                self.live_hosts().len()
            )),
        );
        alert.add_responses(&[("cancel", "Annulla"), ("update", "Aggiorna profilo")]);
        alert.set_response_appearance("update", adw::ResponseAppearance::Suggested);
        alert.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        let parent = parent.clone();
        let id = profile.id.clone();
        let offline = offline.to_vec();
        alert.connect_response(None, move |_, response| {
            if response != "update" {
                return;
            }
            let Some(this) = weak.upgrade() else { return };
            this.add_offline(&offline);
            if this.update_profile(&id) {
                parent.close();
            }
        });
        alert.present(Some(&self.window));
    }
}

/// "ip · vendor · mac".
fn host_line(host: &Host) -> String {
    std::iter::once(host.ip.as_str())
        .chain(host.vendor.as_deref())
        .chain(host.mac.as_deref())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn host_row(host: &Host, mark: &str, tint: &str, notes: &[String]) -> adw::ActionRow {
    let subtitle = std::iter::once(host_line(host))
        .chain(notes.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n");
    let row = adw::ActionRow::builder()
        .title(glib_escape(
            &labels::display_name(host).unwrap_or_else(|| host.ip.clone()),
        ))
        .subtitle(glib_escape(&subtitle))
        .subtitle_selectable(true)
        .build();
    let icon = gtk::Image::from_icon_name(labels::device_icon(host.device_type));
    icon.add_css_class("dim-label");
    row.add_prefix(&icon);
    // Prefixes stack leftwards: the mark ends up first.
    let mark = gtk::Image::from_icon_name(mark);
    mark.add_css_class(tint);
    row.add_prefix(&mark);
    row
}

fn offline_row(host: &Host, notes: &[String], wake: detail::WakeFn) -> adw::ActionRow {
    let row = host_row(host, "system-shutdown-symbolic", "error", notes);
    row.add_suffix(&detail::wake_button(host, true, wake));
    row
}

fn change_row(change: &HostChange, mark: &str, tint: &str) -> adw::ActionRow {
    host_row(&change.after, mark, tint, &descriptions(change))
}

/// One line per difference, for display.
fn descriptions(c: &HostChange) -> Vec<String> {
    let dash = |s: &Option<String>| s.clone().unwrap_or_else(|| "—".into());
    let mut lines = Vec::new();
    if c.came_back {
        lines.push("Era spento quando è stato salvato il profilo".to_string());
    }
    if c.ip_changed {
        lines.push(format!("IP: {} → {}", c.before.ip, c.after.ip));
    }
    if c.mac_changed {
        lines.push(format!(
            "MAC: {} → {}",
            dash(&c.before.mac),
            dash(&c.after.mac)
        ));
    }
    if c.vendor_changed {
        lines.push(format!(
            "Produttore: {} → {}",
            dash(&c.before.vendor),
            dash(&c.after.vendor)
        ));
    }
    if c.device_type_changed {
        lines.push(format!(
            "Tipo: {} → {}",
            labels::device_label(c.before.device_type),
            labels::device_label(c.after.device_type)
        ));
    }
    if !c.opened_ports.is_empty() {
        lines.push(format!(
            "Porte aperte in più: {}",
            ports(&c.opened_ports, &c.after)
        ));
    }
    if !c.closed_ports.is_empty() {
        lines.push(format!(
            "Porte non più aperte: {}",
            ports(&c.closed_ports, &c.before)
        ));
    }
    if !c.added_hostnames.is_empty() {
        lines.push(format!("Nuovi nomi: {}", c.added_hostnames.join(", ")));
    }
    if !c.removed_hostnames.is_empty() {
        lines.push(format!(
            "Nomi scomparsi: {}",
            c.removed_hostnames.join(", ")
        ));
    }
    lines
}

/// "443 (https), 8080" — the service name when the scan knew it.
fn ports(numbers: &[u16], host: &Host) -> String {
    numbers
        .iter()
        .map(|n| {
            match host
                .open_ports
                .iter()
                .find(|p| p.number == *n)
                .and_then(|p| p.service.as_deref())
            {
                Some(service) => format!("{n} ({service})"),
                None => n.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Row titles and subtitles are Pango markup.
fn glib_escape(s: &str) -> String {
    gtk::glib::markup_escape_text(s).to_string()
}
