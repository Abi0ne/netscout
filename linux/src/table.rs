//! The host table: a `GtkColumnView` over a `gio::ListStore` of [`Row`]s,
//! filtered by device type and search text, with the device notes in their
//! own column ([`Notes`]).
//!
//! Sorting works like the macOS app's: a click on a column title sorts by
//! that column alone (again: reversed); Shift+click adds the column after
//! the others, then reverses it, then takes it out. With several columns
//! each title shows its rank and direction ("Nome ²▼"). A double click on
//! the divider between two titles fits the column on its left to its
//! content.
//!
//! Rows are `BoxedAnyObject`s replaced on update, so GTK binds them again.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use netscout_core::{device_key, host_matches, DeviceType, Host};

use crate::labels;

/// One table row: a device found up, or one known from a profile that is off.
pub struct Row {
    pub host: Host,
    pub offline: bool,
}

impl Row {
    /// Unique id: the IP for a live device, `off-<mac>` for one that is off.
    pub fn id(&self) -> String {
        if self.offline {
            offline_id(&self.host)
        } else {
            self.host.ip.clone()
        }
    }
}

/// Stable id of a device that is off (it has no live IP of its own).
pub fn offline_id(host: &Host) -> String {
    format!(
        "off-{}",
        host.mac
            .as_deref()
            .map(str::to_lowercase)
            .unwrap_or_else(|| host.ip.clone())
    )
}

/// The `Row` behind a list item.
pub fn row_of(obj: &glib::Object) -> std::cell::Ref<'_, Row> {
    obj.downcast_ref::<glib::BoxedAnyObject>()
        .expect("table items are BoxedAnyObject<Row>")
        .borrow::<Row>()
}

/// What the table shows: a device type (sidebar) and a search query, which
/// also looks at the devices' notes (by `device_key`) when there are some.
#[derive(Default)]
pub struct FilterState {
    pub device_type: Option<DeviceType>,
    pub query: String,
    pub notes: HashMap<String, String>,
}

impl FilterState {
    fn matches(&self, host: &Host) -> bool {
        if self.device_type.is_some_and(|t| t != host.device_type) {
            return false;
        }
        let note = self.notes.get(&device_key(host)).cloned();
        host_matches(host, note, self.query.clone())
    }
}

/// The notes column: the note on screen for a device, what to do when the
/// user edits it, and whether it can be edited now (a scan that belongs to
/// no profile only shows dashes).
pub struct Notes {
    pub get: Box<dyn Fn(&Host) -> String>,
    pub set: NoteSetter,
    pub editable: Box<dyn Fn() -> bool>,
}

pub type NoteSetter = Box<dyn Fn(&Host, String)>;

/// How a column orders two rows.
type RowOrder = Box<dyn Fn(&Row, &Row) -> Ordering>;

/// A table column: title, how to compare two rows, and the text it shows
/// (for fitting the column to its content).
struct Column {
    title: &'static str,
    view: gtk::ColumnViewColumn,
    cmp: RowOrder,
    text: Box<dyn Fn(&Row) -> String>,
    /// Room taken besides the text (an icon, the pencil).
    extra: i32,
}

/// The sort: columns (by index) and their direction, most important first.
type SortKeys = Rc<RefCell<Vec<(usize, bool)>>>;

pub struct Table {
    pub view: gtk::ColumnView,
    pub store: gio::ListStore,
    pub selection: gtk::SingleSelection,
    pub filter: gtk::CustomFilter,
    pub filtered: gtk::FilterListModel,
}

impl Table {
    /// Bind every row again (after notes changed elsewhere). GTK keeps the
    /// cells of an item it already shows, so each row becomes a new item;
    /// the selection is put back.
    pub fn refresh_rows(&self) {
        let selected = self.selection.selected_item().map(|obj| row_of(&obj).id());
        let fresh: Vec<glib::BoxedAnyObject> = (0..self.store.n_items())
            .filter_map(|i| self.store.item(i))
            .map(|obj| {
                let row = row_of(&obj);
                glib::BoxedAnyObject::new(Row {
                    host: row.host.clone(),
                    offline: row.offline,
                })
            })
            .collect();
        self.store.splice(0, self.store.n_items(), &fresh);
        if let Some(id) = selected {
            let pos = (0..self.selection.n_items()).find(|&i| {
                self.selection
                    .item(i)
                    .is_some_and(|obj| row_of(&obj).id() == id)
            });
            if let Some(pos) = pos {
                self.selection.set_selected(pos);
            }
        }
    }
}

pub fn build(filter_state: Rc<RefCell<FilterState>>, notes: Option<Notes>) -> Table {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let filter =
        gtk::CustomFilter::new(move |obj| filter_state.borrow().matches(&row_of(obj).host));
    let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));

    let view = gtk::ColumnView::new(None::<gtk::SingleSelection>);
    view.set_show_column_separators(false);
    view.set_reorderable(false);
    view.add_css_class("data-table");

    let columns: Rc<RefCell<Vec<Column>>> = Rc::new(RefCell::new(Vec::new()));
    let add = |title: &'static str,
               width: i32,
               expand: bool,
               extra: i32,
               cell: Box<dyn Fn(&Row) -> gtk::Widget>,
               cmp: RowOrder,
               text: Box<dyn Fn(&Row) -> String>| {
        let view_column = add_column(&view, title, width, expand, cell);
        columns.borrow_mut().push(Column {
            title,
            view: view_column,
            cmp,
            text,
            extra,
        });
    };
    add(
        "Tipo",
        110,
        false,
        26,
        Box::new(type_cell),
        Box::new(|a: &Row, b: &Row| {
            labels::device_label(a.host.device_type).cmp(labels::device_label(b.host.device_type))
        }),
        Box::new(|r: &Row| labels::device_label(r.host.device_type).to_string()),
    );
    add(
        "IP",
        118,
        false,
        0,
        Box::new(|row: &Row| text_cell(&row.host.ip, &["numeric"], row.offline)),
        Box::new(|a: &Row, b: &Row| {
            labels::ip_value(&a.host.ip).cmp(&labels::ip_value(&b.host.ip))
        }),
        Box::new(|r: &Row| r.host.ip.clone()),
    );
    add(
        "Nome",
        120,
        true,
        0,
        Box::new(|row: &Row| {
            optional_cell(labels::display_name(&row.host), &[], row.offline).upcast()
        }),
        Box::new(|a: &Row, b: &Row| {
            labels::display_name(&a.host).cmp(&labels::display_name(&b.host))
        }),
        Box::new(|r: &Row| labels::display_name(&r.host).unwrap_or_else(|| "—".into())),
    );
    add(
        "Produttore",
        130,
        true,
        0,
        Box::new(|row: &Row| optional_cell(row.host.vendor.clone(), &[], row.offline).upcast()),
        Box::new(|a: &Row, b: &Row| a.host.vendor.cmp(&b.host.vendor)),
        Box::new(|r: &Row| r.host.vendor.clone().unwrap_or_else(|| "—".into())),
    );
    add(
        "MAC",
        150,
        false,
        0,
        Box::new(|row: &Row| {
            let l = optional_cell(row.host.mac.clone(), &["monospace"], row.offline);
            l.add_css_class("dim-label");
            l.upcast()
        }),
        Box::new(|a: &Row, b: &Row| a.host.mac.cmp(&b.host.mac)),
        Box::new(|r: &Row| r.host.mac.clone().unwrap_or_else(|| "—".into())),
    );
    add(
        "Latenza",
        75,
        false,
        0,
        Box::new(latency_cell),
        Box::new(|a: &Row, b: &Row| rtt_key(a).total_cmp(&rtt_key(b))),
        Box::new(|r: &Row| {
            if r.offline {
                "Spento".into()
            } else {
                r.host.rtt_ms.map_or("—".into(), |v| format!("{v:.1} ms"))
            }
        }),
    );
    add(
        "Porte",
        90,
        true,
        0,
        Box::new(ports_cell),
        Box::new(|a: &Row, b: &Row| a.host.open_ports.len().cmp(&b.host.open_ports.len())),
        Box::new(|r: &Row| {
            r.host
                .open_ports
                .iter()
                .map(|p| p.number.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        }),
    );
    if let Some(notes) = notes {
        let notes = Rc::new(notes);
        let (cell_notes, cmp_notes, text_notes) = (notes.clone(), notes.clone(), notes);
        add(
            "Note",
            150,
            true,
            26,
            Box::new(move |row: &Row| note_cell(row, &cell_notes)),
            Box::new(move |a: &Row, b: &Row| {
                (cmp_notes.get)(&a.host).cmp(&(cmp_notes.get)(&b.host))
            }),
            Box::new(move |r: &Row| (text_notes.get)(&r.host)),
        );
    }

    // Our own sorter, fed by the sort keys; the column view's own one only
    // reports clicks. Devices that are off always come after the others.
    let keys: SortKeys = Rc::new(RefCell::new(vec![(1, true)]));
    let (k, c) = (keys.clone(), columns.clone());
    let by_keys = gtk::CustomSorter::new(move |a, b| {
        let (a, b) = (row_of(a), row_of(b));
        let columns = c.borrow();
        for &(i, ascending) in k.borrow().iter() {
            let order = (columns[i].cmp)(&a, &b);
            if order != Ordering::Equal {
                return if ascending { order } else { order.reverse() }.into();
            }
        }
        gtk::Ordering::Equal
    });
    let sorter = gtk::MultiSorter::new();
    sorter.append(sorter_by(|a, b| a.offline.cmp(&b.offline)));
    sorter.append(by_keys.clone());
    let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter));
    let selection = gtk::SingleSelection::new(Some(sorted));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    view.set_model(Some(&selection));

    for column in columns.borrow().iter() {
        // Any sorter makes the title clickable; the order is ours.
        column
            .view
            .set_sorter(Some(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal)));
    }
    install_sorting(&view, columns.clone(), keys, by_keys);
    install_fit(&view, columns, selection.clone());

    Table {
        view,
        store,
        selection,
        filter,
        filtered,
    }
}

/// Header clicks drive `keys` (see the module docs).
fn install_sorting(
    view: &gtk::ColumnView,
    columns: Rc<RefCell<Vec<Column>>>,
    keys: SortKeys,
    by_keys: gtk::CustomSorter,
) {
    // Whether Shift was down at the last press: the click itself reaches us
    // only as a change of the column view's sorter.
    let shift = Rc::new(Cell::new(false));
    let press = gtk::GestureClick::new();
    press.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = shift.clone();
    press.connect_pressed(move |gesture, _, _, _| {
        s.set(
            gesture
                .current_event_state()
                .contains(gdk::ModifierType::SHIFT_MASK),
        );
    });
    view.add_controller(press);

    let applying = Rc::new(Cell::new(false));
    let show = {
        let (view, columns, keys, applying) = (
            view.clone(),
            columns.clone(),
            keys.clone(),
            applying.clone(),
        );
        move || {
            let columns = columns.borrow();
            let keys = keys.borrow();
            let &(first, ascending) = keys.first().expect("at least one sort key");
            applying.set(true);
            view.sort_by_column(Some(&columns[first].view), sort_type(ascending));
            applying.set(false);
            for (i, column) in columns.iter().enumerate() {
                column.view.set_title(Some(&title(column.title, i, &keys)));
            }
        }
    };
    show();
    let Some(view_sorter) = view.sorter().and_downcast::<gtk::ColumnViewSorter>() else {
        return;
    };
    view_sorter.connect_changed(move |s, _| {
        if applying.get() {
            return;
        }
        let Some(clicked) = s.primary_sort_column() else {
            return;
        };
        let index = columns.borrow().iter().position(|c| c.view == clicked);
        let Some(index) = index else { return };
        let new = sort(&keys.borrow(), index, shift.get());
        *keys.borrow_mut() = new;
        show();
        by_keys.changed(gtk::SorterChange::Different);
    });
}

fn sort_type(ascending: bool) -> gtk::SortType {
    if ascending {
        gtk::SortType::Ascending
    } else {
        gtk::SortType::Descending
    }
}

/// The sort after a click on column `column` (`adding`: with Shift).
fn sort(keys: &[(usize, bool)], column: usize, adding: bool) -> Vec<(usize, bool)> {
    if !adding {
        // The column alone, reversed if it already led.
        let ascending = match keys.first() {
            Some(&(c, asc)) if c == column => !asc,
            _ => true,
        };
        return vec![(column, ascending)];
    }
    let mut keys = keys.to_vec();
    match keys.iter().position(|&(c, _)| c == column) {
        Some(i) if keys[i].1 => keys[i].1 = false,
        Some(i) if keys.len() > 1 => {
            keys.remove(i);
        }
        Some(i) => keys[i].1 = true,
        None => keys.push((column, true)),
    }
    keys
}

/// A column title: with several sort columns, each one shows its rank and,
/// after the first (whose arrow GTK draws), its direction.
fn title(base: &str, column: usize, keys: &[(usize, bool)]) -> String {
    let Some(i) = keys
        .iter()
        .position(|&(c, _)| c == column)
        .filter(|_| keys.len() > 1)
    else {
        return base.to_string();
    };
    const DIGITS: [&str; 10] = ["⁰", "¹", "²", "³", "⁴", "⁵", "⁶", "⁷", "⁸", "⁹"];
    let rank: String = (i + 1)
        .to_string()
        .chars()
        .filter_map(|d| d.to_digit(10).map(|d| DIGITS[d as usize]))
        .collect();
    if i == 0 {
        format!("{base} {rank}")
    } else {
        format!("{base} {rank}{}", if keys[i].1 { "▲" } else { "▼" })
    }
}

/// A double click on the divider between two titles fits the column on its
/// left to the widest value shown, or to its title if wider.
fn install_fit(
    view: &gtk::ColumnView,
    columns: Rc<RefCell<Vec<Column>>>,
    selection: gtk::SingleSelection,
) {
    let press = gtk::GestureClick::new();
    press.set_propagation_phase(gtk::PropagationPhase::Capture);
    let v = view.clone();
    press.connect_pressed(move |gesture, n, x, y| {
        if n != 2 {
            return;
        }
        // The header is the column view's first child; its children are the
        // titles, in column order.
        let Some(header) = v.first_child() else {
            return;
        };
        let Some(bounds) = header.compute_bounds(&v) else {
            return;
        };
        if y < bounds.y() as f64 || y > (bounds.y() + bounds.height()) as f64 {
            return;
        }
        let mut titles = Vec::new();
        let mut child = header.first_child();
        while let Some(w) = child {
            if w.is_visible() {
                titles.push(w.clone());
            }
            child = w.next_sibling();
        }
        let columns = columns.borrow();
        for (i, title) in titles.iter().enumerate().take(columns.len()) {
            let Some(b) = title.compute_bounds(&v) else {
                continue;
            };
            let edge = (b.x() + b.width()) as f64;
            if (x - edge).abs() <= 5.0 {
                let column = &columns[i];
                let content = (0..selection.n_items())
                    .filter_map(|r| selection.item(r))
                    .map(|obj| text_width(&v, &(column.text)(&row_of(&obj))))
                    .max()
                    .unwrap_or(0)
                    + column.extra;
                // Cells and titles have some padding; the title also has
                // the sort arrow.
                let title = text_width(&v, &column.view.title().unwrap_or_default()) + 28;
                column.view.set_fixed_width(content.max(title) + 24);
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }
        }
    });
    view.add_controller(press);
}

fn text_width(widget: &impl IsA<gtk::Widget>, text: &str) -> i32 {
    widget.create_pango_layout(Some(text)).pixel_size().0
}

fn rtt_key(row: &Row) -> f64 {
    if row.offline {
        f64::INFINITY
    } else {
        row.host.rtt_ms.unwrap_or(f64::MAX)
    }
}

fn sorter_by(cmp: impl Fn(&Row, &Row) -> Ordering + 'static) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| cmp(&row_of(a), &row_of(b)).into())
}

/// Append a column whose cells are rebuilt by `cell` on every bind (rows are
/// few and cells are small, so no widget recycling bookkeeping is needed).
fn add_column(
    view: &gtk::ColumnView,
    title: &str,
    width: i32,
    expand: bool,
    cell: Box<dyn Fn(&Row) -> gtk::Widget>,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let Some(obj) = item.item() {
            item.set_child(Some(&cell(&row_of(&obj))));
        }
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(None::<&gtk::Widget>);
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_resizable(true);
    column.set_fixed_width(width);
    column.set_expand(expand);
    view.append_column(&column);
    column
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn text_cell(text: &str, classes: &[&str], dim: bool) -> gtk::Widget {
    let l = label(text, classes);
    if dim {
        l.add_css_class("dim-label");
    }
    l.upcast()
}

/// A value or a dimmed dash.
fn optional_cell(text: Option<String>, classes: &[&str], dim: bool) -> gtk::Label {
    let l = label(text.as_deref().unwrap_or("—"), classes);
    if dim || text.is_none() {
        l.add_css_class("dim-label");
    }
    l
}

fn type_cell(row: &Row) -> gtk::Widget {
    let t = row.host.device_type;
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.append(&gtk::Image::from_icon_name(labels::device_icon(t)));
    b.append(&label(labels::device_label(t), &[]));
    if row.offline || t == DeviceType::Unknown {
        b.add_css_class("dim-label");
    }
    b.upcast()
}

fn latency_cell(row: &Row) -> gtk::Widget {
    if row.offline {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        b.append(&gtk::Image::from_icon_name("system-shutdown-symbolic"));
        b.append(&label("Spento", &["caption-heading"]));
        b.add_css_class("warning");
        return b.upcast();
    }
    let text = row.host.rtt_ms.map(|r| format!("{r:.1} ms"));
    let l = optional_cell(text, &["numeric"], false);
    l.add_css_class("dim-label");
    l.upcast()
}

fn ports_cell(row: &Row) -> gtk::Widget {
    let ports = &row.host.open_ports;
    let numbers: Vec<String> = ports.iter().map(|p| p.number.to_string()).collect();
    let l = label(&numbers.join(" "), &["dim-label", "numeric"]);
    if !ports.is_empty() {
        let tip: Vec<String> = ports
            .iter()
            .map(|p| format!("{} {}", p.number, p.service.as_deref().unwrap_or("")))
            .collect();
        l.set_tooltip_text(Some(&tip.join("\n")));
    }
    l.upcast()
}

/// The device's note: plain text that edits in place. On the row under the
/// mouse a pencil shows it can be written (click it, or the text, to edit;
/// see `style.css`). Without a profile to write to, just the note or a dash.
fn note_cell(row: &Row, notes: &Rc<Notes>) -> gtk::Widget {
    let text = (notes.get)(&row.host);
    if !(notes.editable)() {
        let l = optional_cell((!text.is_empty()).then(|| text.clone()), &[], true);
        if !text.is_empty() {
            l.set_tooltip_text(Some(&text));
        }
        return l.upcast();
    }
    let entry = gtk::Entry::builder()
        .text(text)
        .placeholder_text("Aggiungi una nota")
        .has_frame(false)
        .hexpand(true)
        .build();
    entry.add_css_class("note-entry");
    let host = row.host.clone();
    let n = notes.clone();
    entry.connect_changed(move |e| (n.set)(&host, e.text().to_string()));
    let pencil = gtk::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text(if entry.text().is_empty() {
            "Aggiungi una nota"
        } else {
            "Modifica la nota"
        })
        .valign(gtk::Align::Center)
        .build();
    pencil.add_css_class("flat");
    pencil.add_css_class("note-pencil");
    let e = entry.clone();
    pencil.connect_clicked(move |_| {
        e.grab_focus();
        e.set_position(-1);
    });
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    b.append(&entry);
    b.append(&pencil);
    b.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_sorts_by_one_column() {
        assert_eq!(sort(&[(1, true)], 2, false), vec![(2, true)]);
        assert_eq!(sort(&[(2, true)], 2, false), vec![(2, false)]);
        assert_eq!(sort(&[(2, true), (1, true)], 1, false), vec![(1, true)]);
    }

    #[test]
    fn shift_click_adds_reverses_removes() {
        let k = sort(&[(1, true)], 3, true);
        assert_eq!(k, vec![(1, true), (3, true)]);
        let k = sort(&k, 3, true);
        assert_eq!(k, vec![(1, true), (3, false)]);
        let k = sort(&k, 3, true);
        assert_eq!(k, vec![(1, true)]);
        // The last column is reversed, never removed.
        assert_eq!(sort(&k, 1, true), vec![(1, false)]);
        assert_eq!(sort(&[(1, false)], 1, true), vec![(1, true)]);
    }

    #[test]
    fn titles_show_rank_and_direction() {
        let keys = [(2, true), (1, false)];
        assert_eq!(title("Nome", 2, &keys), "Nome ¹");
        assert_eq!(title("IP", 1, &keys), "IP ²▼");
        assert_eq!(title("MAC", 4, &keys), "MAC");
        assert_eq!(title("Nome", 2, &[(2, true)]), "Nome");
    }
}
