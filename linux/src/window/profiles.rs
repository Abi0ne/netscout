//! The saved-profiles page (list of profiles and, for the selected one, the
//! devices it recorded) and saving a finished scan as a profile.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use netscout_core::{ProfileSummary, SavedProfile};

use super::Window;
use crate::labels;
use crate::table::{self, FilterState, Row, Table};

pub(super) struct ProfilesWidgets {
    pub root: gtk::Widget,
    pub split: adw::OverlaySplitView,
    list: gtk::ListBox,
    list_stack: gtk::Stack,
    detail_stack: gtk::Stack,
    title: gtk::Label,
    subtitle: gtk::Label,
    compare: gtk::Button,
    rename: gtk::Button,
    delete: gtk::Button,
    table: Table,
}

pub(super) fn build() -> ProfilesWidgets {
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    list.set_selection_mode(gtk::SelectionMode::Single);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_child(Some(&list));
    let list_stack = gtk::Stack::new();
    list_stack.add_named(&scroller, Some("list"));
    list_stack.add_named(
        &adw::StatusPage::builder()
            .icon_name("document-save-symbolic")
            .title("Nessun profilo")
            .description("A scansione finita, usa «Salva come profilo…».")
            .css_classes(["compact"])
            .build(),
        Some("empty"),
    );
    list_stack.add_css_class("sidebar-pane");

    let title = gtk::Label::new(None);
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.add_css_class("title-2");
    let subtitle = gtk::Label::new(None);
    subtitle.set_xalign(0.0);
    subtitle.set_wrap(true);
    subtitle.add_css_class("dim-label");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    text.append(&title);
    text.append(&subtitle);

    let compare = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("view-dual-symbolic")
                .label("Confronta con la scansione")
                .build(),
        )
        .valign(gtk::Align::Center)
        .build();
    compare.add_css_class("suggested-action");
    let rename = gtk::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("Rinomina il profilo")
        .valign(gtk::Align::Center)
        .build();
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Elimina il profilo")
        .valign(gtk::Align::Center)
        .build();
    delete.add_css_class("destructive-action");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_margin_top(16);
    header.set_margin_bottom(16);
    header.set_margin_start(16);
    header.set_margin_end(16);
    header.append(&text);
    header.append(&compare);
    header.append(&rename);
    header.append(&delete);

    let table = table::build(Rc::new(RefCell::new(FilterState::default())));
    let table_scroller = gtk::ScrolledWindow::new();
    table_scroller.set_child(Some(&table.view));
    table_scroller.set_vexpand(true);
    let detail = gtk::Box::new(gtk::Orientation::Vertical, 0);
    detail.append(&header);
    detail.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    detail.append(&table_scroller);

    let detail_stack = gtk::Stack::new();
    detail_stack.add_named(
        &adw::StatusPage::builder()
            .icon_name("document-save-symbolic")
            .title("Nessun profilo selezionato")
            .description("Seleziona un profilo dalla lista.")
            .build(),
        Some("none"),
    );
    detail_stack.add_named(&detail, Some("profile"));

    let split = adw::OverlaySplitView::builder()
        .sidebar(&list_stack)
        .content(&detail_stack)
        .min_sidebar_width(250.0)
        .max_sidebar_width(360.0)
        .sidebar_width_fraction(0.24)
        .build();
    ProfilesWidgets {
        root: split.clone().upcast(),
        split,
        list,
        list_stack,
        detail_stack,
        title,
        subtitle,
        compare,
        rename,
        delete,
        table,
    }
}

impl Window {
    pub(super) fn connect_profile_signals(self: &Rc<Self>) {
        let p = &self.w.profiles;
        let weak = Rc::downgrade(self);
        p.list
            .connect_selected_rows_changed(super::with(&weak, |this, _: &gtk::ListBox| {
                this.show_profile();
            }));
        p.compare
            .connect_clicked(super::with(&weak, |this, _: &gtk::Button| {
                if let Some(id) = this.selected_profile_id() {
                    this.compare_with(&id);
                }
            }));
        p.rename
            .connect_clicked(super::with(&weak, |this, _: &gtk::Button| {
                this.ask_rename_profile()
            }));
        p.delete
            .connect_clicked(super::with(&weak, |this, _: &gtk::Button| {
                this.ask_delete_profile()
            }));
    }

    pub(super) fn refresh_profiles(&self) {
        let Some(store) = &self.profile_store else {
            return;
        };
        match store.clone().list() {
            Ok(list) => *self.profiles.borrow_mut() = list,
            Err(e) => self.show_error(&format!("Impossibile leggere i profili: {e}")),
        }
        self.rebuild_profile_list();
        self.rebuild_compare_menu();
    }

    fn rebuild_profile_list(&self) {
        let p = &self.w.profiles;
        let keep = self.selected_profile_id();
        p.list.remove_all();
        let profiles = self.profiles.borrow();
        for summary in profiles.iter() {
            let row = profile_row(summary);
            p.list.append(&row);
            if keep.as_deref() == Some(summary.id.as_str()) {
                p.list.select_row(Some(&row));
            }
        }
        p.list_stack
            .set_visible_child_name(if profiles.is_empty() { "empty" } else { "list" });
        drop(profiles);
        self.show_profile();
    }

    fn rebuild_compare_menu(&self) {
        let menu = gio::Menu::new();
        for summary in self.profiles.borrow().iter() {
            let item = gio::MenuItem::new(Some(&summary.name), None);
            item.set_action_and_target_value(Some("win.compare"), Some(&summary.id.to_variant()));
            menu.append_item(&item);
        }
        let any = menu.n_items() > 0;
        self.w.compare_menu.set_menu_model(Some(&menu));
        self.w.compare_menu.set_sensitive(any);
        self.w.compare_menu.set_tooltip_text(Some(if any {
            "Trova le differenze rispetto a un profilo salvato"
        } else {
            "Nessun profilo salvato"
        }));
    }

    fn selected_profile_id(&self) -> Option<String> {
        self.w
            .profiles
            .list
            .selected_row()
            .map(|r| r.widget_name().to_string())
    }

    fn selected_profile(&self) -> Option<ProfileSummary> {
        let id = self.selected_profile_id()?;
        self.profiles.borrow().iter().find(|p| p.id == id).cloned()
    }

    fn show_profile(&self) {
        let p = &self.w.profiles;
        let Some(summary) = self.selected_profile() else {
            p.detail_stack.set_visible_child_name("none");
            return;
        };
        p.title.set_text(&summary.name);
        let updated = summary
            .updated_at
            .map(|t| format!(", aggiornato {}", labels::format_time(t, true)))
            .unwrap_or_default();
        p.subtitle.set_text(&format!(
            "{} · profilo {} · {} dispositivi · salvato {}{updated}",
            summary.target,
            labels::profile_label(summary.scan_profile).to_lowercase(),
            summary.host_count,
            labels::format_time(summary.created_at, true),
        ));
        let finished = self.has_finished_scan();
        p.compare.set_sensitive(finished);
        p.compare.set_tooltip_text(Some(if finished {
            "Trova le differenze tra la scansione corrente e questo profilo"
        } else {
            "Esegui prima una scansione completa"
        }));
        p.table.store.remove_all();
        if let Some(saved) = self.load_profile(&summary.id) {
            let rows = saved
                .hosts
                .into_iter()
                .map(|host| Row {
                    host,
                    offline: false,
                })
                .chain(saved.offline_hosts.into_iter().map(|host| Row {
                    host,
                    offline: true,
                }))
                .map(glib::BoxedAnyObject::new)
                .collect::<Vec<_>>();
            p.table.store.extend_from_slice(&rows);
        }
        p.detail_stack.set_visible_child_name("profile");
    }

    pub(super) fn update_profile_buttons(&self) {
        if self.selected_profile_id().is_some() {
            let finished = self.has_finished_scan();
            self.w.profiles.compare.set_sensitive(finished);
        }
    }

    pub(super) fn load_profile(&self, id: &str) -> Option<SavedProfile> {
        let store = self.profile_store.as_ref()?;
        match store.clone().load(id.to_string()) {
            Ok(p) => Some(p),
            Err(e) => {
                self.show_error(&format!("Impossibile aprire il profilo: {e}"));
                None
            }
        }
    }

    // -----------------------------------------------------------------------
    // Save, rename, delete, update
    // -----------------------------------------------------------------------

    /// Ask for a name and save the finished scan as a new profile.
    pub(super) fn ask_save_profile(self: &Rc<Self>) {
        if !self.has_finished_scan() {
            return;
        }
        let suggested = format!(
            "{} · {}",
            self.scanned_target.borrow(),
            labels::format_time(glib::real_time() / 1000, true)
        );
        let count = self.live_hosts().len();
        let weak = Rc::downgrade(self);
        ask_name(
            &self.window,
            "Salva come profilo",
            &format!(
                "Salva i {count} dispositivi trovati per confrontarli con le prossime scansioni."
            ),
            &suggested,
            "Salva",
            move |name| {
                if let Some(this) = weak.upgrade() {
                    this.save_profile(name);
                }
            },
        );
    }

    fn save_profile(&self, name: &str) {
        let Some(store) = &self.profile_store else {
            return;
        };
        let result = store.clone().save(
            name.to_string(),
            self.scanned_target.borrow().clone(),
            self.scanned_profile.get(),
            self.live_hosts(),
            self.offline_hosts(),
        );
        match result {
            Ok(_) => self.refresh_profiles(),
            Err(e) => self.show_error(&format!("Salvataggio non riuscito: {e}")),
        }
    }

    /// Replace profile `id` with this scan: the devices up and those off.
    pub(super) fn update_profile(&self, id: &str) -> bool {
        let Some(store) = &self.profile_store else {
            return false;
        };
        if !self.has_finished_scan() {
            return false;
        }
        let result = store.clone().update(
            id.to_string(),
            self.scanned_target.borrow().clone(),
            self.scanned_profile.get(),
            self.live_hosts(),
            self.offline_hosts(),
        );
        self.refresh_profiles();
        match result {
            Ok(_) => true,
            Err(e) => {
                self.show_error(&format!("Aggiornamento del profilo non riuscito: {e}"));
                false
            }
        }
    }

    fn ask_rename_profile(self: &Rc<Self>) {
        let Some(summary) = self.selected_profile() else {
            return;
        };
        let weak = Rc::downgrade(self);
        ask_name(
            &self.window,
            "Rinomina profilo",
            "",
            &summary.name,
            "Rinomina",
            move |name| {
                let Some(this) = weak.upgrade() else { return };
                if let Some(store) = &this.profile_store {
                    if let Err(e) = store.clone().rename(summary.id.clone(), name.to_string()) {
                        this.show_error(&format!("Impossibile rinominare il profilo: {e}"));
                    }
                }
                this.refresh_profiles();
            },
        );
    }

    fn ask_delete_profile(self: &Rc<Self>) {
        let Some(summary) = self.selected_profile() else {
            return;
        };
        let dialog = adw::AlertDialog::new(
            Some(&format!("Eliminare il profilo «{}»?", summary.name)),
            Some("L'operazione non si può annullare."),
        );
        dialog.add_responses(&[("cancel", "Annulla"), ("delete", "Elimina")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if response != "delete" {
                return;
            }
            let Some(this) = weak.upgrade() else { return };
            if let Some(store) = &this.profile_store {
                if let Err(e) = store.clone().delete(summary.id.clone()) {
                    this.show_error(&format!("Impossibile eliminare il profilo: {e}"));
                }
            }
            this.w.profiles.list.unselect_all();
            this.refresh_profiles();
        });
        dialog.present(Some(&self.window));
    }
}

fn profile_row(summary: &ProfileSummary) -> gtk::ListBoxRow {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.set_margin_top(4);
    b.set_margin_bottom(4);
    let name = gtk::Label::new(Some(&summary.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("heading");
    b.append(&name);
    let off = if summary.offline_count > 0 {
        format!(" · {} spenti", summary.offline_count)
    } else {
        String::new()
    };
    for line in [
        labels::format_time(summary.created_at, true),
        format!(
            "{} · {} dispositivi{off}",
            summary.target, summary.host_count
        ),
    ] {
        let l = gtk::Label::new(Some(&line));
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.add_css_class("caption");
        l.add_css_class("dim-label");
        b.append(&l);
    }
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&b));
    row.set_widget_name(&summary.id);
    row
}

/// A dialog with a name field; `done` gets the trimmed, non-empty name.
fn ask_name(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    body: &str,
    initial: &str,
    confirm: &str,
    done: impl Fn(&str) + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(title), (!body.is_empty()).then_some(body));
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", "Annulla"), ("ok", confirm)]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");
    let d = dialog.clone();
    entry.connect_changed(move |e| d.set_response_enabled("ok", !e.text().trim().is_empty()));
    dialog.set_response_enabled("ok", !initial.trim().is_empty());
    let e = entry.clone();
    dialog.connect_response(None, move |_, response| {
        let name = e.text().trim().to_string();
        if response == "ok" && !name.is_empty() {
            done(&name);
        }
    });
    dialog.present(Some(parent));
    entry.grab_focus();
}
