//! Preferenze (Ctrl+,): the DNS servers the deep profile asks for the
//! devices' names, like the macOS app's Settings → Scansione.

use adw::prelude::*;

use crate::config;

const GROUP: &str = "scan";
const KEY: &str = "name_servers";

/// The IPv4 addresses in the setting (spaces, commas or semicolons between).
pub fn name_servers() -> Vec<String> {
    words(&config::get(GROUP, KEY).unwrap_or_default())
        .into_iter()
        .filter(|w| is_ipv4(w))
        .collect()
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_ipv4(text: &str) -> bool {
    text.parse::<std::net::Ipv4Addr>().is_ok()
}

pub fn show(parent: &impl IsA<gtk::Widget>) {
    let entry = adw::EntryRow::builder()
        .title("Server DNS")
        .text(config::get(GROUP, KEY).unwrap_or_default())
        .build();
    entry.set_tooltip_text(Some("es. 10.0.0.10, 10.0.0.11"));
    let warning = gtk::Label::new(None);
    warning.set_xalign(0.0);
    warning.set_wrap(true);
    warning.add_css_class("caption");
    warning.add_css_class("warning");
    warning.set_margin_top(6);
    let check = {
        let warning = warning.clone();
        move |text: &str| {
            let invalid: Vec<String> = words(text).into_iter().filter(|w| !is_ipv4(w)).collect();
            warning.set_visible(!invalid.is_empty());
            warning.set_text(&format!(
                "Non sono indirizzi IPv4, verranno ignorati: {}",
                invalid.join(", ")
            ));
        }
    };
    check(&entry.text());
    entry.connect_changed(move |e| {
        let text = e.text();
        config::set(GROUP, KEY, text.trim());
        check(&text);
    });

    let group = adw::PreferencesGroup::builder()
        .title("Nomi dei dispositivi (scansione approfondita)")
        .description("Con il profilo Approfondita, e con «Scansione approfondita» sul singolo dispositivo, NetScout cerca i nomi anche dove mDNS e NetBIOS non arrivano, ad esempio da un'altra VLAN: chiede il nome ai PC Windows (condivisione file e Desktop remoto), legge i certificati delle pagine web e interroga direttamente i server DNS. Oltre ai dispositivi della rete scansionata che fanno da DNS, chiede a quelli indicati qui: in una rete aziendale, di solito i domain controller.")
        .build();
    group.add(&entry);
    group.add(&warning);
    let page = adw::PreferencesPage::builder()
        .title("Scansione")
        .icon_name("network-transmit-receive-symbolic")
        .build();
    page.add(&group);
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Preferenze");
    dialog.add(&page);
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_checks_addresses() {
        assert_eq!(
            words("10.0.0.1, 10.0.0.2;x  y"),
            ["10.0.0.1", "10.0.0.2", "x", "y"]
        );
        assert!(is_ipv4("10.0.0.1"));
        assert!(!is_ipv4("10.0.0"));
        assert!(!is_ipv4("dc.local"));
    }
}
