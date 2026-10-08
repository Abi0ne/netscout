//! Guida di NetScout (F1): every feature, one topic per page; and the About
//! dialog.

use adw::prelude::*;
use gtk::glib;

enum Block {
    Heading(&'static str),
    /// Light Markdown: **bold**, *italics*, `code`.
    Text(&'static str),
    Bullets(&'static [&'static str]),
    Note(&'static str),
}

struct Topic {
    title: &'static str,
    icon: &'static str,
    blocks: &'static [Block],
}

use Block::*;

const TOPICS: &[Topic] = &[
    Topic {
        title: "Panoramica",
        icon: "help-about-symbolic",
        blocks: &[
            Text("NetScout trova i dispositivi collegati alla tua rete locale e ti dice cosa sono: indirizzo IP, MAC, produttore, nome, tipo di dispositivo, latenza e porte aperte."),
            Text("Funziona **senza privilegi di amministratore**: non serve la password né `sudo`. Il ping usa i socket ICMP non privilegiati di Linux (`net.ipv4.ping_group_range`, attivi di serie su Ubuntu, Fedora e Debian); se non sono disponibili la scansione trova comunque i dispositivi con le connessioni TCP."),
            Heading("La finestra"),
            Bullets(&[
                "**Scansione** — la scheda principale: a sinistra i dispositivi trovati, divisi per tipo, e in fondo la rete da analizzare con il pulsante di avvio (se la finestra è bassa, questi controlli si stringono su due righe per lasciare spazio all'elenco); al centro la tabella dei dispositivi; a destra la scheda del dispositivo selezionato. Il pulsante in alto a sinistra mostra o nasconde la barra laterale, quello accanto al campo di ricerca la scheda.",
                "**Profili salvati** — le scansioni che hai salvato, da consultare e confrontare con la rete di oggi.",
                "Si passa dall'una all'altra con i due pulsanti in alto, all'inizio della tabella, o con Ctrl+1 e Ctrl+2.",
                "**Barra in basso** — la versione di NetScout e gli avvisi di cambio rete.",
            ]),
            Heading("Il menu principale"),
            Text("Il pulsante **☰** in cima alla barra laterale apre le **Preferenze** (Ctrl+,), questa guida (F1), le informazioni su NetScout e il comando per uscire (Ctrl+Q)."),
        ],
    },
    Topic {
        title: "Scansione della rete",
        icon: "network-transmit-receive-symbolic",
        blocks: &[
            Heading("Scegliere cosa analizzare"),
            Text("Nella sezione **Rete** della barra laterale il menu mostra le interfacce attive del computer (Wi‑Fi, Ethernet, …) con la loro rete: all'avvio è già scelta quella con il router. Puoi anche scrivere tu la destinazione nel campo di testo:"),
            Bullets(&[
                "un indirizzo singolo: `192.168.1.10`",
                "una rete in notazione CIDR: `192.168.1.0/24`",
                "un intervallo sull'ultimo numero: `192.168.1.10-200`",
                "un intervallo completo: `192.168.1.10-192.168.1.200`",
                "più destinazioni separate da virgola: `192.168.1.0/24, 10.0.0.5`",
            ]),
            Text("Premendo Invio nel campo la scansione parte subito."),
            Heading("I profili di scansione"),
            Bullets(&[
                "**Rapida** — controlla solo 7 porte comuni (SSH, DNS, web, condivisione file) con attese brevi: ideale per vedere in pochi secondi chi è acceso.",
                "**Standard** — 20 porte, tra cui FTP, Telnet, RDP, stampanti, telecamere (RTSP), AirPlay e MQTT. È il compromesso consigliato.",
                "**Approfondita** — circa 40 porte, compresi posta, database, VNC, Plex e NAS, con attese più lunghe: più lenta ma più completa. Cerca anche i nomi che da un'altra VLAN non arriverebbero (vedi sotto, **Nome**).",
            ]),
            Heading("Durante la scansione"),
            Text("Premi **Avvia scansione** (o Ctrl+R). I dispositivi compaiono nella tabella man mano che rispondono; sotto il pulsante vedi la fase in corso (*Ricerca dispositivi*, *Nomi e produttori*, *Porte*), quanti dispositivi sono attivi, le porte aperte trovate e il tempo trascorso. **Ferma** interrompe la scansione tenendo quanto trovato."),
            Heading("Come vengono riconosciuti i dispositivi"),
            Bullets(&[
                "**Presenza** — ping e tentativi di connessione su porte comuni: basta una risposta, anche un rifiuto, per sapere che il dispositivo è acceso.",
                "**Produttore** — dal MAC, con una tabella dei produttori inclusa nell'app. Se il MAC è *privato (casuale)*, come fanno telefoni e tablet, il produttore non si può sapere.",
                "**Nome** — da DNS inverso, mDNS/Avahi (computer, telefoni, stampanti, altoparlanti) e NetBIOS (Windows e Samba). mDNS e NetBIOS di solito non attraversano le VLAN: con il profilo **Approfondita** NetScout chiede il nome anche ai PC Windows (condivisione file, porta 445, e Desktop remoto, 3389), legge i certificati delle pagine web (443, 8443, 5001) e interroga direttamente i server DNS della rete scansionata e quelli indicati nelle **Preferenze** (ad esempio i domain controller).",
                "**Tipo** — dedotto da produttore, nome, porte aperte e servizi annunciati (router, computer, stampante, telecamera, NAS, TV, console, IoT…).",
            ]),
            Heading("Cambi di rete"),
            Text("NetScout segue la rete mentre è aperto: se ti colleghi a un'altra Wi‑Fi, attacchi un cavo o cambia l'indirizzo, l'elenco delle interfacce si aggiorna da solo e la barra in basso mostra «Rete cambiata». Se stavi usando una rete rilevata automaticamente che non c'è più, la destinazione passa alla nuova; una destinazione scritta a mano non viene toccata."),
        ],
    },
    Topic {
        title: "Elenco dei dispositivi",
        icon: "view-list-symbolic",
        blocks: &[
            Text("La tabella mostra per ogni dispositivo **tipo, IP, nome, produttore, MAC, latenza, porte aperte e note**. Clicca sull'intestazione di una colonna per ordinare."),
            Heading("Ordinare per più colonne"),
            Bullets(&[
                "**Clic** sull'intestazione — ordina solo per quella colonna; un altro clic inverte l'ordine.",
                "**Maiusc+clic** — aggiunge la colonna all'ordinamento dopo le altre; un altro Maiusc+clic la inverte, un terzo la toglie.",
                "Le colonne che partecipano mostrano la priorità e il verso, per esempio **Produttore ¹** e **Nome ²▼**: prima per produttore, a parità di produttore per nome.",
            ]),
            Heading("Larghezza delle colonne"),
            Text("Fai **doppio clic sul separatore** a destra dell'intestazione di una colonna per adattarne la larghezza al testo più lungo, intestazione compresa, tra i dispositivi elencati."),
            Heading("Filtrare e cercare"),
            Bullets(&[
                "Nella sezione **Dispositivi**, in cima alla barra laterale, trovi quanti dispositivi ci sono per ciascun tipo: clicca un tipo per vedere solo quelli, ricliccalo (o scegli **Tutti**) per tornare all'elenco completo.",
                "Il campo di ricerca in alto a destra, sopra la scheda del dispositivo e largo quanto lei (Ctrl+F), filtra per IP, nome, produttore, MAC o nota.",
            ]),
            Heading("Note"),
            Text("La colonna **Note** mostra le note del profilo della rete riconosciuta, o di quello in cui hai salvato la scansione. Passando col mouse su una riga compare una matita: cliccala per scrivere. Vedi *Profili e confronti*."),
            Heading("Dispositivi spenti"),
            Text("Dopo un confronto con un profilo puoi aggiungere alla tabella i dispositivi salvati ma non trovati: compaiono in grigio, in fondo, con l'etichetta **Spento**. Dalla loro scheda puoi accenderli o toglierli dall'elenco con **Dimentica dispositivo spento**."),
        ],
    },
    Topic {
        title: "Scheda del dispositivo",
        icon: "sidebar-show-right-symbolic",
        blocks: &[
            Text("Seleziona un dispositivo per aprire la sua scheda a destra. Il pulsante con l'icona del pannello, accanto al campo di ricerca, la mostra o la nasconde; nelle finestre strette la scheda si apre sopra la tabella."),
            Heading("Cosa contiene"),
            Bullets(&[
                "**Nota** — la nota del dispositivo, la stessa della colonna Note: si scrive qui, su più righe, e si salva con **Salva**.",
                "**Indirizzo IP, MAC, produttore e nomi** — tutti selezionabili per copiarli.",
                "**Latenza** — il tempo di risposta al ping; «Non risponde al ping» se il dispositivo ha risposto solo su qualche porta.",
                "**Porte aperte** — numero e servizio di ciascuna porta.",
            ]),
            Heading("Azioni sulle porte"),
            Bullets(&[
                "**Apri** — sulle porte web (80, 443, 8080, 8443, …) apre l'interfaccia del dispositivo nel browser.",
                "**Terminale** — sulle porte SSH e Telnet apre una sessione nel terminale. Vedi *Connessioni remote*.",
                "**Remmina** — sulla porta RDP (3389) apre il desktop remoto, se Remmina è installato.",
            ]),
            Heading("Scansione approfondita"),
            Text("Il pulsante **Scansione approfondita**, nella scheda sotto la nota, rianalizza solo quel dispositivo con il profilo approfondito, per scoprire porte e servizi che la scansione rapida o standard non controlla, e i nomi che arrivano anche da un'altra VLAN. Su un dispositivo spento diventa **Riscansiona**, per verificare se nel frattempo si è acceso."),
        ],
    },
    Topic {
        title: "Connessioni remote",
        icon: "utilities-terminal-symbolic",
        blocks: &[
            Text("Accanto alle porte **SSH (22)** e **Telnet (23)** il pulsante **Terminale** apre una sessione nel terminale del desktop (Ptyxis, Console, GNOME Terminal, Konsole, …)."),
            Bullets(&[
                "Si apre una finestrella dove inserire **utente** e **password**. L'utente viene ricordato per ogni dispositivo.",
                "La password **non viene salvata**: serve solo per l'accesso e viene cancellata appena la sessione parte. Lasciala vuota per scriverla nel terminale o per usare una chiave SSH.",
                "Al primo collegamento SSH la chiave del dispositivo viene accettata da sola; se in seguito cambia, il collegamento viene rifiutato per sicurezza.",
                "La finestra del terminale si chiude quando la sessione finisce.",
            ]),
            Note("Per inserire la password nella finestrella serve `expect` (`sudo apt install expect`); senza, la password si scrive nel terminale. Telnet si installa con `sudo apt install telnet`."),
            Text("Le sessioni **RDP** (Desktop remoto, porta 3389) si aprono con Remmina (`sudo apt install remmina`)."),
        ],
    },
    Topic {
        title: "Accensione (Wake-on-LAN)",
        icon: "system-shutdown-symbolic",
        blocks: &[
            Text("Un dispositivo spento salvato in un profilo si può accendere a distanza con il **Wake-on-LAN**: seleziona il dispositivo spento e premi **Accendi (Wake-on-LAN)**."),
            Bullets(&[
                "NetScout invia il «pacchetto magico» al MAC del dispositivo; se il Wake-on-LAN è attivo, si accende in qualche secondo.",
                "Per verificare premi **Riscansiona**: se risponde, torna tra i dispositivi accesi.",
                "Serve il MAC: senza, il pulsante è disattivato.",
                "I dispositivi con MAC privato (telefoni, tablet) di solito non si accendono così.",
            ]),
            Note("Il Wake-on-LAN va abilitato sul dispositivo stesso (nel BIOS/UEFI o nelle impostazioni di risparmio energia) e funziona di norma solo sulla stessa rete."),
        ],
    },
    Topic {
        title: "Profili e confronti",
        icon: "document-save-symbolic",
        blocks: &[
            Text("Un **profilo** è una fotografia della rete: l'elenco dei dispositivi trovati in una scansione, con le loro porte. I profili sono salvati in `~/.local/share/NetScout/Profiles`."),
            Heading("Salvare"),
            Text("A scansione finita, nella sezione **Risultato** premi **Salva come profilo…** (Ctrl+S) e dai un nome. I profili si trovano nella scheda **Profili salvati**, dove puoi consultarli, rinominarli o eliminarli (clic destro sul profilo)."),
            Heading("Confrontare"),
            Text("Usa **Confronta con profilo** nella barra laterale (o il pulsante in alto nella scheda Profili salvati) per vedere cosa è cambiato rispetto a una scansione salvata:"),
            Bullets(&[
                "**Nuovi dispositivi** — presenti ora, assenti nel profilo.",
                "**Spenti o scomparsi** — nel profilo, ma non trovati ora.",
                "**Di nuovo accesi** — erano spenti quando hai salvato il profilo.",
                "**Stesso MAC, IP diverso** — lo stesso dispositivo ha cambiato indirizzo.",
                "**Stesso IP, MAC diverso** — all'indirizzo ora c'è un altro dispositivo.",
                "**Altre modifiche** — nome, produttore, tipo o porte aperte cambiati.",
            ]),
            Text("I dispositivi vengono riconosciuti prima dal MAC, poi dall'IP."),
            Heading("Dal confronto puoi"),
            Bullets(&[
                "**Aggiungere gli spenti alla scansione** — per vederli in tabella e accenderli con il Wake-on-LAN.",
                "**Aggiornare il profilo** — sostituirlo con la scansione attuale; i dispositivi spenti restano nel profilo.",
            ]),
            Heading("Note sui dispositivi"),
            Text("Nella tabella di un profilo, la colonna **Note** si modifica con un clic: scrivi dove si trova un dispositivo, a chi appartiene o qualsiasi altra informazione. Le note restano legate al dispositivo (dal MAC) anche quando aggiorni il profilo."),
            Text("La colonna **Note** c'è anche nella tabella della scansione: mostra le note del profilo della rete riconosciuta, o di quello in cui hai appena salvato la scansione. Passando col mouse su una riga compare una matita nella colonna: cliccala per scrivere. La stessa nota si modifica anche nella scheda del dispositivo selezionato, a destra. Le modifiche, su tutti i dispositivi, vanno nel profilo: si salvano con **Salva** (Ctrl+S) nella sezione **Note** della barra laterale o nella scheda. Se la rete non è in nessun profilo, salva prima la scansione come profilo. Salvando come nuovo profilo una scansione riconosciuta, le sue note passano anche al nuovo profilo."),
            Text("Nella scheda Profili salvati le modifiche si salvano con **Salva note** (Ctrl+S) o si scartano con **Annulla modifiche**; finché non le salvi il profilo ha un pallino arancione nell'elenco. Se chiudi la finestra o esci con note non salvate, NetScout chiede se salvarle."),
            Heading("Cercare"),
            Text("Il campo di ricerca in alto (Ctrl+F) filtra i profili per nome o rete scansionata, oppure per i dispositivi che contengono (IP, nome, produttore, MAC o nota). Se a corrispondere sono solo alcuni dispositivi, la tabella del profilo mostra solo quelli."),
            Heading("Rete riconosciuta"),
            Text("Durante una scansione NetScout confronta i dispositivi trovati con i profili salvati e, se riconosce la rete, te lo segnala proponendo di confrontarla con il profilo."),
            Bullets(&[
                "Basta ritrovare il **router** (lo stesso MAC del gateway) in un profilo.",
                "Senza il router servono almeno **3 dispositivi** del profilo, e almeno la metà di quelli riconoscibili.",
                "Contano solo identificatori stabili: il MAC assegnato dal produttore e, se il MAC manca, l'identificativo UPnP. IP, nomi e MAC privati (telefoni, tablet) non vengono usati.",
                "Se due profili di reti diverse corrispondono allo stesso modo, NetScout non propone nulla.",
            ]),
            Heading("Esportare in CSV"),
            Text("Il pulsante **Esporta CSV** in alto (o il clic destro su un profilo) salva un file con un dispositivo per riga: profilo, rete, date, stato, IP, nomi, tipo, produttore, MAC, latenza, porte aperte, servizi e note. Il file usa il punto e virgola come separatore e si apre direttamente in LibreOffice Calc o Excel con le lettere accentate corrette."),
        ],
    },
    Topic {
        title: "Preferenze",
        icon: "preferences-system-symbolic",
        blocks: &[
            Text("Le **Preferenze** (Ctrl+, o dal menu **☰**) hanno la sezione **Scansione**."),
            Heading("Server DNS"),
            Text("Con il profilo **Approfondita**, e con **Scansione approfondita** sul singolo dispositivo, NetScout chiede i nomi dei dispositivi anche direttamente ai server DNS: a quelli della rete scansionata e a quelli scritti qui, separati da virgole. In una rete aziendale segmentata in VLAN, di solito sono i domain controller, che conoscono i nomi di tutte le reti."),
        ],
    },
    Topic {
        title: "Scorciatoie",
        icon: "preferences-desktop-keyboard-shortcuts-symbolic",
        blocks: &[Bullets(&[
            "**Ctrl+R** — avvia o ferma la scansione",
            "**Ctrl+F** — cerca nell'elenco dei dispositivi o dei profili",
            "**Ctrl+S** — salva le note modificate, o la scansione come profilo",
            "**Ctrl+1**, **Ctrl+2** — passa alla scheda Scansione o Profili salvati",
            "**Ctrl+,** — apri le Preferenze",
            "**F1** — apri questa guida",
            "**Invio** nel campo destinazione — avvia la scansione",
            "**Ctrl+W** — chiudi la finestra",
            "**Ctrl+Q** — esci",
        ])],
    },
];

pub fn show(parent: &impl IsA<gtk::Widget>) {
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    for topic in TOPICS {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        b.append(&gtk::Image::from_icon_name(topic.icon));
        let l = gtk::Label::new(Some(topic.title));
        l.set_xalign(0.0);
        b.append(&l);
        list.append(&b);
    }
    let sidebar_scroller = gtk::ScrolledWindow::new();
    sidebar_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    sidebar_scroller.set_child(Some(&list));
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(
        &adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .build(),
    );
    sidebar_view.set_content(Some(&sidebar_scroller));
    let sidebar = adw::NavigationPage::new(&sidebar_view, "Guida di NetScout");

    let content_bin = adw::Bin::new();
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&adw::HeaderBar::new());
    content_view.set_content(Some(&content_bin));
    let content = adw::NavigationPage::new(&content_view, TOPICS[0].title);

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar)
        .content(&content)
        .min_sidebar_width(200.0)
        .build();

    let show_topic = {
        let content = content.clone();
        let split = split.clone();
        move |i: usize| {
            let topic = &TOPICS[i];
            content.set_title(topic.title);
            content_bin.set_child(Some(&topic_page(topic)));
            split.set_show_content(true);
        }
    };
    show_topic(0);
    list.select_row(list.row_at_index(0).as_ref());
    list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            show_topic(row.index() as usize);
        }
    });

    let dialog = adw::Dialog::builder()
        .title("Guida di NetScout")
        .content_width(900)
        .content_height(620)
        .width_request(360)
        .height_request(320)
        .child(&split)
        .build();
    let bp =
        adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 600sp").expect("valid"));
    bp.add_setter(&split, "collapsed", Some(&true.to_value()));
    dialog.add_breakpoint(bp);
    dialog.present(Some(parent));
}

fn topic_page(topic: &Topic) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 14);
    b.set_margin_top(12);
    b.set_margin_bottom(28);
    b.set_margin_start(28);
    b.set_margin_end(28);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = gtk::Image::from_icon_name(topic.icon);
    icon.set_pixel_size(28);
    icon.add_css_class("accent");
    title.append(&icon);
    let l = gtk::Label::new(Some(topic.title));
    l.add_css_class("title-1");
    title.append(&l);
    b.append(&title);
    for block in topic.blocks {
        match block {
            Heading(text) => {
                let l = markup_label(&glib::markup_escape_text(text));
                l.add_css_class("title-4");
                l.set_margin_top(6);
                b.append(&l);
            }
            Text(text) => b.append(&markup_label(&markdown(text))),
            Bullets(items) => {
                let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
                for item in *items {
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let dot = gtk::Label::new(Some("•"));
                    dot.set_valign(gtk::Align::Start);
                    dot.add_css_class("dim-label");
                    row.append(&dot);
                    let l = markup_label(&markdown(item));
                    l.set_hexpand(true);
                    row.append(&l);
                    list.append(&row);
                }
                b.append(&list);
            }
            Note(text) => {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                row.add_css_class("card");
                row.add_css_class("help-note");
                let icon = gtk::Image::from_icon_name("dialog-information-symbolic");
                icon.set_valign(gtk::Align::Start);
                icon.add_css_class("warning");
                row.append(&icon);
                let l = markup_label(&markdown(text));
                l.set_hexpand(true);
                row.append(&l);
                b.append(&row);
            }
        }
    }
    let clamp = adw::Clamp::builder().maximum_size(720).child(&b).build();
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_child(Some(&clamp));
    scroller.upcast()
}

fn markup_label(markup: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(markup);
    l.set_wrap(true);
    l.set_xalign(0.0);
    l.set_selectable(true);
    l
}

/// Light Markdown (`**bold**`, `*italics*`, `` `code` ``) to Pango markup.
fn markdown(text: &str) -> String {
    let escaped = glib::markup_escape_text(text).to_string();
    let mut out = String::with_capacity(escaped.len() + 32);
    let (mut bold, mut italic, mut code) = (false, false, false);
    let mut chars = escaped.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                out.push_str(if code { "</tt>" } else { "<tt>" });
                code = !code;
            }
            '*' if !code && chars.peek() == Some(&'*') => {
                chars.next();
                out.push_str(if bold { "</b>" } else { "<b>" });
                bold = !bold;
            }
            '*' if !code => {
                out.push_str(if italic { "</i>" } else { "<i>" });
                italic = !italic;
            }
            _ => out.push(c),
        }
    }
    out
}

pub fn show_about(parent: &impl IsA<gtk::Widget>) {
    let about = adw::AboutDialog::builder()
        .application_name("NetScout")
        .application_icon(crate::APP_ID)
        .version(netscout_core::VERSION)
        .developer_name("Giulio Mancarella")
        .comments("Scanner della rete locale veloce e senza privilegi.")
        .website("https://github.com/Abi0ne/netscout")
        .issue_url("https://github.com/Abi0ne/netscout/issues")
        .license_type(gtk::License::MitX11)
        .build();
    about.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::markdown;

    #[test]
    fn markdown_to_pango() {
        assert_eq!(
            markdown("**a** *b* `c*d` <e>"),
            "<b>a</b> <i>b</i> <tt>c*d</tt> &lt;e&gt;"
        );
    }

    #[test]
    fn every_topic_parses_as_markup() {
        for topic in super::TOPICS {
            for block in topic.blocks {
                let texts: Vec<&str> = match block {
                    super::Block::Text(t) | super::Block::Note(t) => vec![t],
                    super::Block::Bullets(items) => items.to_vec(),
                    super::Block::Heading(_) => vec![],
                };
                for t in texts {
                    let m = markdown(t);
                    assert!(gtk::pango::parse_markup(&m, '\0').is_ok(), "{m}");
                }
            }
        }
    }
}
