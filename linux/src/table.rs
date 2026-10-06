//! The host table: a sortable `GtkColumnView` over a `gio::ListStore` of
//! [`Row`]s, filtered by device type and search text.
//!
//! Rows are `BoxedAnyObject`s mutated in place on update (followed by
//! `items_changed`), so the selection survives the engine's stream of updates.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use netscout_core::{DeviceType, Host};

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

/// What the table shows: a device type (sidebar) and a search query.
#[derive(Default)]
pub struct FilterState {
    pub device_type: Option<DeviceType>,
    pub query: String,
}

impl FilterState {
    fn matches(&self, host: &Host) -> bool {
        if self.device_type.is_some_and(|t| t != host.device_type) {
            return false;
        }
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        [
            &host.ip,
            host.mac.as_deref().unwrap_or(""),
            host.vendor.as_deref().unwrap_or(""),
        ]
        .into_iter()
        .chain(host.hostnames.iter().map(String::as_str))
        .any(|f| f.to_lowercase().contains(&q))
    }
}

pub struct Table {
    pub view: gtk::ColumnView,
    pub store: gio::ListStore,
    pub selection: gtk::SingleSelection,
    pub filter: gtk::CustomFilter,
    pub filtered: gtk::FilterListModel,
}

pub fn build(filter_state: Rc<RefCell<FilterState>>) -> Table {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let filter =
        gtk::CustomFilter::new(move |obj| filter_state.borrow().matches(&row_of(obj).host));
    let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));

    let view = gtk::ColumnView::new(None::<gtk::SingleSelection>);
    view.set_show_column_separators(false);
    view.set_reorderable(false);
    view.add_css_class("data-table");

    // Devices that are off always come after the others, whatever the sort.
    let sorter = gtk::MultiSorter::new();
    sorter.append(sorter_by(|a, b| a.offline.cmp(&b.offline)));
    if let Some(columns) = view.sorter() {
        sorter.append(columns);
    }
    let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter));
    let selection = gtk::SingleSelection::new(Some(sorted));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    view.set_model(Some(&selection));

    add_column(&view, "Tipo", 115, false, type_cell, |a, b| {
        labels::device_label(a.host.device_type).cmp(labels::device_label(b.host.device_type))
    });
    let ip = add_column(
        &view,
        "IP",
        105,
        false,
        |row| text_cell(&row.host.ip, &["numeric"], row.offline),
        |a, b| labels::ip_value(&a.host.ip).cmp(&labels::ip_value(&b.host.ip)),
    );
    add_column(
        &view,
        "Nome",
        120,
        true,
        |row| optional_cell(labels::display_name(&row.host), &[], row.offline).upcast(),
        |a, b| labels::display_name(&a.host).cmp(&labels::display_name(&b.host)),
    );
    add_column(
        &view,
        "Produttore",
        120,
        true,
        |row| optional_cell(row.host.vendor.clone(), &[], row.offline).upcast(),
        |a, b| a.host.vendor.cmp(&b.host.vendor),
    );
    add_column(
        &view,
        "MAC",
        165,
        false,
        |row| {
            let l = optional_cell(row.host.mac.clone(), &["monospace"], row.offline);
            l.add_css_class("dim-label");
            l.upcast()
        },
        |a, b| a.host.mac.cmp(&b.host.mac),
    );
    add_column(&view, "Latenza", 75, false, latency_cell, |a, b| {
        rtt_key(a).total_cmp(&rtt_key(b))
    });
    add_column(&view, "Porte", 80, true, ports_cell, |a, b| {
        a.host.open_ports.len().cmp(&b.host.open_ports.len())
    });
    view.sort_by_column(Some(&ip), gtk::SortType::Ascending);

    Table {
        view,
        store,
        selection,
        filter,
        filtered,
    }
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
    cell: impl Fn(&Row) -> gtk::Widget + 'static,
    cmp: impl Fn(&Row, &Row) -> Ordering + 'static,
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
    column.set_sorter(Some(&sorter_by(cmp)));
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
