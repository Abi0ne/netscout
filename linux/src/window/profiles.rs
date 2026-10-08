//! The saved-profiles tab (the list of profiles in the sidebar, searchable,
//! and, for the selected one, the devices it recorded with the user's
//! notes), saving a finished scan as a profile, exporting profiles as CSV,
//! and the notes, of a profile or of the scan.
//!
//! Notes are edited as drafts (`note_drafts`, by profile) and written only
//! by "Salva"; closing the window with drafts asks what to do with them. The
//! scan's notes are those of the profile it was saved as, else the one its
//! network was recognized as.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use netscout_core::{
    device_key, match_profile, profiles_csv, Host, ProfileMatch, ProfileSummary, SavedProfile,
};

use super::{NoteSource, Window};
use crate::labels;
use crate::table::{self, FilterState, Row, Table};

pub(super) struct ProfilesWidgets {
    /// The list, for the sidebar, and the selected profile, for the middle.
    pub sidebar: gtk::Widget,
    pub content: gtk::Widget,
    pub list: gtk::ListBox,
    list_stack: gtk::Stack,
    detail_stack: gtk::Stack,
    title: gtk::Label,
    subtitle: gtk::Label,
    pub table: Table,
    filter: Rc<RefCell<FilterState>>,
    unsaved_box: gtk::Box,
    save_notes: gtk::Button,
    discard_notes: gtk::Button,
    /// The "unsaved notes" dot of each listed profile, by id.
    dots: RefCell<HashMap<String, gtk::Widget>>,
}

pub(super) fn build(notes: table::Notes) -> ProfilesWidgets {
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
    list_stack.add_named(
        &adw::StatusPage::builder()
            .icon_name("edit-find-symbolic")
            .title("Nessun risultato")
            .description("Nessun profilo corrisponde alla ricerca.")
            .css_classes(["compact"])
            .build(),
        Some("no-results"),
    );

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

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_margin_top(16);
    header.set_margin_bottom(16);
    header.set_margin_start(16);
    header.set_margin_end(16);
    header.append(&text);

    // Shown while the notes have changes not saved yet.
    let unsaved_label = gtk::Label::new(Some("Note modificate, non ancora salvate"));
    unsaved_label.set_xalign(0.0);
    unsaved_label.set_hexpand(true);
    unsaved_label.add_css_class("warning");
    let discard_notes = gtk::Button::with_label("Annulla modifiche");
    let save_notes = gtk::Button::with_label("Salva note");
    save_notes.add_css_class("suggested-action");
    save_notes.set_tooltip_text(Some("Salva le note di questo profilo (Ctrl+S)"));
    let unsaved_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    unsaved_box.set_margin_bottom(12);
    unsaved_box.set_margin_start(16);
    unsaved_box.set_margin_end(16);
    unsaved_box.append(&unsaved_label);
    unsaved_box.append(&discard_notes);
    unsaved_box.append(&save_notes);
    unsaved_box.set_visible(false);

    let filter = Rc::new(RefCell::new(FilterState::default()));
    let table = table::build(filter.clone(), Some(notes));
    let table_scroller = gtk::ScrolledWindow::new();
    table_scroller.set_child(Some(&table.view));
    table_scroller.set_vexpand(true);
    let detail = gtk::Box::new(gtk::Orientation::Vertical, 0);
    detail.append(&header);
    detail.append(&unsaved_box);
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

    ProfilesWidgets {
        sidebar: list_stack.clone().upcast(),
        content: detail_stack.clone().upcast(),
        list,
        list_stack,
        detail_stack,
        title,
        subtitle,
        table,
        filter,
        unsaved_box,
        save_notes,
        discard_notes,
        dots: RefCell::new(HashMap::new()),
    }
}

impl Window {
    pub(super) fn connect_profile_signals(self: &Rc<Self>) {
        let p = &self.w.profiles;
        let weak = Rc::downgrade(self);
        p.list
            .connect_selected_rows_changed(super::with(&weak, |this, _: &gtk::ListBox| {
                this.show_profile();
                this.rebuild_export_menu();
            }));
        // Right click on a profile: rename, export, delete.
        let menu = gio::Menu::new();
        menu.append(Some("Rinomina…"), Some("win.rename-profile"));
        menu.append(Some("Esporta CSV…"), Some("win.export-selected"));
        menu.append(Some("Elimina…"), Some("win.delete-profile"));
        // On the list's container: a list box takes every child for a row.
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&p.list_stack);
        popover.set_has_arrow(false);
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let (list, stack) = (p.list.clone(), p.list_stack.clone());
        click.connect_pressed(move |_, _, x, y| {
            let Some(row) = list.row_at_y(y as i32) else {
                return;
            };
            list.select_row(Some(&row));
            let point = gtk::graphene::Point::new(x as f32, y as f32);
            let point = list.compute_point(&stack, &point).unwrap_or(point);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                point.x() as i32,
                point.y() as i32,
                1,
                1,
            )));
            popover.popup();
        });
        p.list.add_controller(click);
        p.save_notes
            .connect_clicked(super::with(&weak, |this, _: &gtk::Button| {
                this.save_selected_notes();
            }));
        p.discard_notes
            .connect_clicked(super::with(&weak, |this, _: &gtk::Button| {
                if let Some(id) = this.selected_profile_id() {
                    this.discard_notes(&id);
                }
            }));
        self.w.profile_search.connect_search_changed(super::with(
            &weak,
            |this, _: &gtk::SearchEntry| {
                this.rebuild_profile_list();
            },
        ));
    }

    pub(super) fn refresh_profiles(&self) {
        let Some(store) = &self.profile_store else {
            return;
        };
        match (store.clone().list(), store.clone().load_all()) {
            (Ok(list), Ok(all)) => {
                *self.profiles.borrow_mut() = list;
                *self.saved.borrow_mut() = all;
            }
            (Err(e), _) | (_, Err(e)) => {
                self.show_error(&format!("Impossibile leggere i profili: {e}"))
            }
        }
        let saved = self.saved.borrow();
        self.note_drafts
            .borrow_mut()
            .retain(|id, _| saved.iter().any(|p| &p.id == id));
        drop(saved);
        self.rebuild_profile_list();
        self.rebuild_compare_menu();
    }

    fn search_query(&self) -> String {
        self.w.profile_search.text().trim().to_string()
    }

    /// How profile `id` matches the search, with the notes on screen.
    fn profile_match(&self, id: &str) -> ProfileMatch {
        match self.profile_with_drafts(id) {
            Some(p) => match_profile(&p, self.search_query()),
            None => ProfileMatch::None,
        }
    }

    fn rebuild_profile_list(&self) {
        let p = &self.w.profiles;
        let keep = self.selected_profile_id();
        p.list.remove_all();
        p.dots.borrow_mut().clear();
        let profiles = self.profiles.borrow();
        let mut shown = 0;
        for summary in profiles.iter() {
            if self.profile_match(&summary.id) == ProfileMatch::None {
                continue;
            }
            shown += 1;
            let (row, dot) = profile_row(summary);
            dot.set_visible(self.note_drafts.borrow().contains_key(&summary.id));
            p.dots.borrow_mut().insert(summary.id.clone(), dot);
            p.list.append(&row);
            if keep.as_deref() == Some(summary.id.as_str()) {
                p.list.select_row(Some(&row));
            }
        }
        p.list_stack
            .set_visible_child_name(match (profiles.len(), shown) {
                (0, _) => "empty",
                (_, 0) => "no-results",
                _ => "list",
            });
        drop(profiles);
        self.show_profile();
        self.rebuild_export_menu();
    }

    /// "Esporta CSV": the selected profile, the ones found by the search,
    /// all of them.
    fn rebuild_export_menu(&self) {
        let menu = gio::Menu::new();
        if self.selected_profile_id().is_some() {
            menu.append(Some("Profilo selezionato…"), Some("win.export-selected"));
        }
        let all = self.profiles.borrow().len();
        if !self.search_query().is_empty() {
            let found = self
                .profiles
                .borrow()
                .iter()
                .filter(|p| self.profile_match(&p.id) != ProfileMatch::None)
                .count();
            menu.append(
                Some(&format!("Profili trovati ({found})…")),
                (found > 0).then_some("win.export-listed"),
            );
        }
        menu.append(
            Some(&format!("Tutti i profili ({all})…")),
            Some("win.export-all"),
        );
        self.w.export_menu.set_menu_model(Some(&menu));
        self.w.export_menu.set_sensitive(all > 0);
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

    pub(super) fn selected_profile_id(&self) -> Option<String> {
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
            self.update_profile_buttons();
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
        self.update_profile_buttons();
        p.table.store.remove_all();
        {
            // The profile's devices narrow to those matching the search,
            // unless the profile itself (name, network) matches.
            let mut filter = p.filter.borrow_mut();
            filter.notes = self.notes_of(&summary.id);
            filter.query = if self.profile_match(&summary.id) == ProfileMatch::Devices {
                self.search_query()
            } else {
                String::new()
            };
        }
        p.table.filter.changed(gtk::FilterChange::Different);
        let saved = self
            .saved
            .borrow()
            .iter()
            .find(|s| s.id == summary.id)
            .cloned();
        if let Some(saved) = saved {
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
        self.update_unsaved_ui();
    }

    // -----------------------------------------------------------------------
    // Notes
    // -----------------------------------------------------------------------

    /// The profiles tab's notes column, editing the selected profile.
    pub(super) fn notes_column(weak: Rc<RefCell<std::rc::Weak<Window>>>) -> table::Notes {
        let (w2, w3) = (weak.clone(), weak.clone());
        table::Notes {
            get: Box::new(move |host| {
                let Some(this) = weak.borrow().upgrade() else {
                    return String::new();
                };
                let id = this.selected_profile_id().unwrap_or_default();
                this.note_of(&id, host)
            }),
            set: Box::new(move |host, text| {
                let Some(this) = w2.borrow().upgrade() else {
                    return;
                };
                if let Some(id) = this.selected_profile_id() {
                    this.set_note(&id, host, text, NoteSource::ProfileTable);
                }
            }),
            editable: Box::new(move || w3.borrow().upgrade().is_some()),
        }
    }

    /// The scan's notes column: the profile the scan belongs to; only
    /// dashes when it belongs to none.
    pub(super) fn scan_notes_column(weak: Rc<RefCell<std::rc::Weak<Window>>>) -> table::Notes {
        let (w2, w3) = (weak.clone(), weak.clone());
        table::Notes {
            get: Box::new(move |host| {
                let Some(this) = weak.borrow().upgrade() else {
                    return String::new();
                };
                match this.scan_notes_profile_id() {
                    Some(id) => this.note_of(&id, host),
                    None => String::new(),
                }
            }),
            set: Box::new(move |host, text| {
                let Some(this) = w2.borrow().upgrade() else {
                    return;
                };
                if let Some(id) = this.scan_notes_profile_id() {
                    this.set_note(&id, host, text, NoteSource::ScanTable);
                }
            }),
            editable: Box::new(move || {
                w3.borrow()
                    .upgrade()
                    .is_some_and(|this| this.scan_notes_profile_id().is_some())
            }),
        }
    }

    /// The profile whose notes the scan shows and edits: the one it was
    /// saved as, else the one its network was recognized as.
    pub(super) fn scan_notes_profile_id(&self) -> Option<String> {
        let saved = self.saved.borrow();
        let exists = |id: &String| saved.iter().any(|p| &p.id == id);
        self.saved_scan_profile
            .borrow()
            .clone()
            .filter(exists)
            .or_else(|| {
                self.recognized
                    .borrow()
                    .as_ref()
                    .map(|m| m.profile_id.clone())
                    .filter(exists)
            })
    }

    pub(super) fn scan_notes_profile(&self) -> Option<ProfileSummary> {
        let id = self.scan_notes_profile_id()?;
        self.profiles.borrow().iter().find(|p| p.id == id).cloned()
    }

    pub(super) fn scan_notes_unsaved(&self) -> bool {
        self.scan_notes_profile_id()
            .is_some_and(|id| self.note_drafts.borrow().contains_key(&id))
    }

    pub(super) fn save_scan_notes(&self) {
        if let Some(id) = self.scan_notes_profile_id() {
            self.save_notes(&id);
        }
    }

    pub(super) fn discard_scan_notes(&self) {
        if let Some(id) = self.scan_notes_profile_id() {
            self.discard_notes(&id);
        }
    }

    /// The profile the scan's notes belong to changed (recognized, saved,
    /// new scan): show its notes everywhere.
    pub(super) fn scan_notes_changed(self: &Rc<Self>) {
        self.filter_state.borrow_mut().notes = self
            .scan_notes_profile_id()
            .map(|id| self.notes_of(&id))
            .unwrap_or_default();
        self.refresh_scan_rows();
        self.update_notes_section();
        self.show_detail();
    }

    /// Every note of profile `id` as on screen: the draft, else the saved ones.
    fn notes_of(&self, id: &str) -> HashMap<String, String> {
        if let Some(draft) = self.note_drafts.borrow().get(id) {
            return draft.clone();
        }
        self.saved
            .borrow()
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.notes.clone())
            .unwrap_or_default()
    }

    pub(super) fn note_of(&self, id: &str, host: &Host) -> String {
        self.notes_of(id)
            .get(&device_key(host))
            .cloned()
            .unwrap_or_default()
    }

    /// The user wrote `text` as the note of `host` in profile `id`, in
    /// `source`: keep it as a draft (dropped when back to the saved notes)
    /// and bring the other views showing that profile up to date.
    pub(super) fn set_note(&self, id: &str, host: &Host, text: String, source: NoteSource) {
        let mut notes = self.notes_of(id);
        notes.insert(device_key(host), text.clone());
        let saved = self
            .saved
            .borrow()
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.notes.clone())
            .unwrap_or_default();
        {
            let mut drafts = self.note_drafts.borrow_mut();
            if clean_notes(&notes) == clean_notes(&saved) {
                drafts.remove(id);
            } else {
                drafts.insert(id.to_string(), notes.clone());
            }
        }
        let profile_shown = self.selected_profile_id().as_deref() == Some(id);
        let scan_shown = self.scan_notes_profile_id().as_deref() == Some(id);
        if profile_shown {
            self.w.profiles.filter.borrow_mut().notes = notes.clone();
            if source != NoteSource::ProfileTable {
                self.w.profiles.table.refresh_rows();
            }
        }
        if scan_shown {
            self.filter_state.borrow_mut().notes = notes;
            if source != NoteSource::ScanTable {
                self.refresh_scan_rows();
            }
            if source != NoteSource::Card {
                self.sync_card_note(host, &text);
            }
        }
        self.update_unsaved_ui();
    }

    /// Show `text` in the card if it shows `host`'s note.
    fn sync_card_note(&self, host: &Host, text: &str) {
        let card = self.card_note.borrow();
        let Some(card) = card.as_ref().filter(|c| c.key == device_key(host)) else {
            return;
        };
        let (start, end) = card.buffer.bounds();
        if card.buffer.text(&start, &end, false) != text {
            self.syncing_note.set(true);
            card.buffer.set_text(text);
            self.syncing_note.set(false);
        }
    }

    /// Drop the drafts of profile `id`.
    fn discard_notes(&self, id: &str) {
        self.note_drafts.borrow_mut().remove(id);
        self.notes_saved_or_dropped(id);
    }

    /// After profile `id`'s notes were saved or their drafts dropped: show
    /// the notes as they are now.
    fn notes_saved_or_dropped(&self, id: &str) {
        if self.selected_profile_id().as_deref() == Some(id) {
            self.show_profile();
        }
        if self.scan_notes_profile_id().as_deref() == Some(id) {
            self.filter_state.borrow_mut().notes = self.notes_of(id);
            self.refresh_scan_rows();
            let card = self.card_note.borrow().as_ref().map(|c| c.key.clone());
            if let Some(key) = card {
                let text = self.notes_of(id).get(&key).cloned().unwrap_or_default();
                let card = self.card_note.borrow();
                if let Some(card) = card.as_ref() {
                    self.syncing_note.set(true);
                    card.buffer.set_text(&text);
                    self.syncing_note.set(false);
                }
            }
        }
        self.update_unsaved_ui();
    }

    /// Every "not saved" mark: the dots in the list, the bar over the
    /// profile's table, the sidebar's "Note" section and the card.
    fn update_unsaved_ui(&self) {
        let p = &self.w.profiles;
        {
            let drafts = self.note_drafts.borrow();
            for (id, dot) in p.dots.borrow().iter() {
                dot.set_visible(drafts.contains_key(id));
            }
            let selected = self.selected_profile_id();
            p.unsaved_box
                .set_visible(selected.is_some_and(|id| drafts.contains_key(&id)));
        }
        self.update_notes_section();
        self.update_card_note();
    }

    /// Profile `id` with the notes on screen (search, export).
    fn profile_with_drafts(&self, id: &str) -> Option<SavedProfile> {
        let mut profile = self.saved.borrow().iter().find(|p| p.id == id).cloned()?;
        profile.notes = clean_notes(&self.notes_of(id));
        Some(profile)
    }

    pub(super) fn save_selected_notes(&self) {
        if let Some(id) = self.selected_profile_id() {
            self.save_notes(&id);
        }
    }

    /// Write the drafts of profile `id`; false if that failed (they are kept).
    fn save_notes(&self, id: &str) -> bool {
        let Some(store) = &self.profile_store else {
            return false;
        };
        let Some(draft) = self.note_drafts.borrow().get(id).cloned() else {
            return true;
        };
        match store.clone().set_notes(id.to_string(), draft.clone()) {
            Ok(()) => {
                let notes = clean_notes(&draft);
                self.note_drafts.borrow_mut().remove(id);
                if let Some(p) = self.saved.borrow_mut().iter_mut().find(|p| p.id == id) {
                    p.notes = notes;
                }
                self.refresh_profiles();
                self.notes_saved_or_dropped(id);
                true
            }
            Err(e) => {
                self.show_error(&format!("Salvataggio delle note non riuscito: {e}"));
                false
            }
        }
    }

    /// Before the window closes: if notes are not saved, ask to save or
    /// discard them. Returns true when the window may close now.
    pub(super) fn confirm_close(self: &Rc<Self>) -> bool {
        let names: Vec<String> = self
            .profiles
            .borrow()
            .iter()
            .filter(|p| self.note_drafts.borrow().contains_key(&p.id))
            .map(|p| format!("«{}»", p.name))
            .collect();
        if names.is_empty() {
            return true;
        }
        let (heading, body) = if names.len() == 1 {
            (
                format!("Salvare le note del profilo {}?", names[0]),
                "Hai modificato delle note senza salvarle.".to_string(),
            )
        } else {
            (
                format!("Salvare le note di {} profili?", names.len()),
                format!(
                    "Hai modificato delle note senza salvarle nei profili {}.",
                    names.join(", ")
                ),
            )
        };
        let dialog = adw::AlertDialog::new(
            Some(&heading),
            Some(&format!(
                "{body} Se non le salvi, le modifiche andranno perse."
            )),
        );
        dialog.add_responses(&[
            ("cancel", "Annulla"),
            ("discard", "Non salvare"),
            ("save", "Salva"),
        ]);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(this) = weak.upgrade() else { return };
            let close = match response {
                "save" => {
                    // Try every profile, even after a failure.
                    let ids: Vec<String> = this.note_drafts.borrow().keys().cloned().collect();
                    let mut ok = true;
                    for id in &ids {
                        ok &= this.save_notes(id);
                    }
                    ok
                }
                "discard" => {
                    this.note_drafts.borrow_mut().clear();
                    true
                }
                _ => false,
            };
            if close {
                this.window.close();
            }
        });
        dialog.present(Some(&self.window));
        false
    }

    // -----------------------------------------------------------------------
    // CSV export
    // -----------------------------------------------------------------------

    pub(super) fn export_selected(self: &Rc<Self>) {
        if let Some(summary) = self.selected_profile() {
            self.export_csv(vec![summary.id], &summary.name);
        }
    }

    /// The profiles the list shows (all of them without a search).
    pub(super) fn export_listed(self: &Rc<Self>) {
        let ids: Vec<String> = self
            .profiles
            .borrow()
            .iter()
            .filter(|p| self.profile_match(&p.id) != ProfileMatch::None)
            .map(|p| p.id.clone())
            .collect();
        if !ids.is_empty() {
            self.export_csv(ids, "profili");
        }
    }

    pub(super) fn export_all(self: &Rc<Self>) {
        let ids: Vec<String> = self
            .profiles
            .borrow()
            .iter()
            .map(|p| p.id.clone())
            .collect();
        if !ids.is_empty() {
            self.export_csv(ids, "profili");
        }
    }

    fn export_csv(self: &Rc<Self>, ids: Vec<String>, name: &str) {
        let date = glib::DateTime::now_local()
            .and_then(|d| d.format("%Y-%m-%d"))
            .map(|s| s.to_string())
            .unwrap_or_default();
        let file_name = format!("NetScout {name} {date}.csv").replace(['/', ':'], "-");
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("CSV"));
        filter.add_suffix("csv");
        filter.add_mime_type("text/csv");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Esporta profili in CSV")
            .initial_name(file_name)
            .filters(&filters)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let (Some(this), Ok(file)) = (weak.upgrade(), result) else {
                return;
            };
            let profiles: Vec<SavedProfile> = ids
                .iter()
                .filter_map(|id| this.profile_with_drafts(id))
                .collect();
            let csv = profiles_csv(&profiles);
            let written = file.path().map(|path| std::fs::write(path, csv));
            match written {
                Some(Ok(())) => {}
                Some(Err(e)) => this.show_error(&format!("Esportazione non riuscita: {e}")),
                None => this.show_error("Esportazione non riuscita: percorso non valido."),
            }
        });
    }

    /// The profiles tab's compare button.
    pub(super) fn update_profile_buttons(&self) {
        let selected = self.selected_profile_id().is_some();
        let finished = self.has_finished_scan();
        let b = &self.w.profile_compare;
        b.set_sensitive(selected && finished);
        b.set_tooltip_text(Some(match (selected, finished) {
            (false, _) => "Seleziona un profilo",
            (true, false) => "Esegui prima una scansione completa",
            (true, true) => "Trova le differenze tra la scansione corrente e questo profilo",
        }));
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

    /// Save the scan as profile `name`. The notes the scan shows (of the
    /// profile it was recognized as) go with it, for the devices it has; the
    /// scan's notes are the new profile's from now on.
    fn save_profile(self: &Rc<Self>, name: &str) {
        let Some(store) = &self.profile_store else {
            return;
        };
        let (live, offline) = (self.live_hosts(), self.offline_hosts());
        let keys: std::collections::HashSet<String> =
            live.iter().chain(&offline).map(device_key).collect();
        let notes: HashMap<String, String> = self
            .scan_notes_profile_id()
            .map(|id| clean_notes(&self.notes_of(&id)))
            .unwrap_or_default()
            .into_iter()
            .filter(|(k, _)| keys.contains(k))
            .collect();
        let result = store.clone().save(
            name.to_string(),
            self.scanned_target.borrow().clone(),
            self.scanned_profile.get(),
            live,
            offline,
        );
        match result {
            Ok(saved) => {
                if !notes.is_empty() {
                    if let Err(e) = store.clone().set_notes(saved.id.clone(), notes) {
                        self.show_error(&format!("Note non copiate nel nuovo profilo: {e}"));
                    }
                }
                *self.saved_scan_profile.borrow_mut() = Some(saved.id);
                self.refresh_profiles();
                self.scan_notes_changed();
            }
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

    pub(super) fn ask_rename_profile(self: &Rc<Self>) {
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

    pub(super) fn ask_delete_profile(self: &Rc<Self>) {
        let Some(summary) = self.selected_profile() else {
            return;
        };
        let body = if self.note_drafts.borrow().contains_key(&summary.id) {
            "Anche le note non salvate andranno perse. L'operazione non si può annullare."
        } else {
            "L'operazione non si può annullare."
        };
        let dialog = adw::AlertDialog::new(
            Some(&format!("Eliminare il profilo «{}»?", summary.name)),
            Some(body),
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
            this.note_drafts.borrow_mut().remove(&summary.id);
            this.w.profiles.list.unselect_all();
            this.refresh_profiles();
        });
        dialog.present(Some(&self.window));
    }
}

/// A profile's list row, and its "unsaved notes" dot (hidden).
fn profile_row(summary: &ProfileSummary) -> (gtk::ListBoxRow, gtk::Widget) {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.set_margin_top(4);
    b.set_margin_bottom(4);
    let name = gtk::Label::new(Some(&summary.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("heading");
    let dot = gtk::Label::new(Some("●"));
    dot.add_css_class("warning");
    dot.add_css_class("caption");
    dot.set_tooltip_text(Some("Note modificate, non salvate"));
    dot.set_visible(false);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    title.append(&name);
    title.append(&dot);
    b.append(&title);
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
    (row, dot.upcast())
}

/// Notes as saved: trimmed, blank ones dropped.
fn clean_notes(notes: &HashMap<String, String>) -> HashMap<String, String> {
    notes
        .iter()
        .map(|(k, v)| (k.clone(), v.trim().to_string()))
        .filter(|(_, v)| !v.is_empty())
        .collect()
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
