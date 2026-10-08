import SwiftUI

/// Aiuto → Guida di NetScout: every feature, one topic per page.
struct HelpView: View {
    @ViewState private var selection: HelpTopic.ID? = HelpTopic.all.first?.id

    var body: some View {
        NavigationSplitView {
            List(HelpTopic.all, selection: $selection) { topic in
                Label(topic.title, systemImage: topic.symbol)
            }
            .navigationSplitViewColumnWidth(min: 200, ideal: 230, max: 300)
        } detail: {
            if let topic = HelpTopic.all.first(where: { $0.id == selection }) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 18) {
                        Label(topic.title, systemImage: topic.symbol)
                            .font(.largeTitle.weight(.semibold))
                        ForEach(topic.blocks.indices, id: \.self) { i in
                            HelpBlockView(block: topic.blocks[i])
                        }
                    }
                    .padding(28)
                    .frame(maxWidth: 720, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .id(topic.id)
            } else {
                ContentUnavailableView("Scegli un argomento", systemImage: "questionmark.circle")
            }
        }
        .frame(minWidth: 760, minHeight: 500)
    }
}

private struct HelpBlockView: View {
    let block: HelpTopic.Block

    var body: some View {
        switch block {
        case .heading(let text):
            Text(text).font(.title3.weight(.semibold)).padding(.top, 6)
        case .text(let text):
            Text(.init(text)).fixedSize(horizontal: false, vertical: true)
        case .bullets(let items):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(items, id: \.self) { item in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("•").foregroundStyle(.secondary)
                        Text(.init(item)).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
        case .note(let text):
            Label { Text(.init(text)).fixedSize(horizontal: false, vertical: true) } icon: {
                Image(systemName: "lightbulb").foregroundStyle(.yellow)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
        }
    }
}

struct HelpTopic: Identifiable {
    enum Block {
        case heading(String)
        /// Markdown (bold, italics, code).
        case text(String)
        case bullets([String])
        case note(String)
    }

    let title: String
    let symbol: String
    let blocks: [Block]
    var id: String { title }

    static let all: [HelpTopic] = [
        HelpTopic(title: "Panoramica", symbol: "info.circle", blocks: [
            .text("NetScout trova i dispositivi collegati alla tua rete locale e ti dice cosa sono: indirizzo IP, MAC, produttore, nome, tipo di dispositivo, latenza e porte aperte."),
            .text("Funziona **senza privilegi di amministratore**: non serve la password né l'installazione di componenti aggiuntivi. La prima volta macOS può chiederti il permesso di accedere alla **rete locale**: va concesso, altrimenti la scansione non trova nulla."),
            .heading("La finestra"),
            .bullets([
                "**Scansione** — la scheda principale: a sinistra i dispositivi trovati, divisi per tipo, e in fondo la rete da analizzare con il pulsante di avvio (se la finestra è bassa, questi controlli si stringono su due righe per lasciare spazio all'elenco); al centro la tabella dei dispositivi; a destra la scheda del dispositivo selezionato. Il pulsante in alto a sinistra mostra o nasconde la barra laterale, quello in alto a destra la scheda.",
                "**Profili salvati** — le scansioni che hai salvato, da consultare e confrontare con la rete di oggi.",
                "Si passa dall'una all'altra con i due pulsanti in alto, all'inizio della tabella, o con ⌘1 e ⌘2.",
                "**Barra in basso** — la versione di NetScout, gli avvisi di cambio rete e lo stato degli aggiornamenti.",
            ]),
            .heading("I menu"),
            .bullets([
                "**NetScout → Impostazioni… (⌘,)** — integrazioni e aggiornamenti.",
                "**NetScout → Controlla aggiornamenti…** — cerca subito una nuova versione.",
                "**Archivio → Avvia/Ferma scansione (⌘R)**.",
                "**Aiuto → Guida di NetScout (⌘?)** — questa guida.",
            ]),
        ]),
        HelpTopic(title: "Scansione della rete", symbol: "dot.radiowaves.left.and.right", blocks: [
            .heading("Scegliere cosa analizzare"),
            .text("Nella sezione **Rete**, in fondo alla barra laterale, il menu mostra le interfacce attive del Mac (Wi‑Fi, Ethernet, …) con la loro rete: all'avvio è già scelta quella con il router. Puoi anche scrivere tu la destinazione nel campo di testo:"),
            .bullets([
                "un indirizzo singolo: `192.168.1.10`",
                "una rete in notazione CIDR: `192.168.1.0/24`",
                "un intervallo sull'ultimo numero: `192.168.1.10-200`",
                "un intervallo completo: `192.168.1.10-192.168.1.200`",
                "più destinazioni separate da virgola: `192.168.1.0/24, 10.0.0.5`",
            ]),
            .text("Premendo Invio nel campo la scansione parte subito."),
            .heading("I profili di scansione"),
            .bullets([
                "**Rapida** — controlla solo 7 porte comuni (SSH, DNS, web, condivisione file) con attese brevi: ideale per vedere in pochi secondi chi è acceso.",
                "**Standard** — 20 porte, tra cui FTP, Telnet, RDP, stampanti, telecamere (RTSP), AirPlay e MQTT. È il compromesso consigliato.",
                "**Approfondita** — circa 40 porte, compresi posta, database, VNC, Plex e NAS, con attese più lunghe: più lenta ma più completa. Cerca anche i nomi che da un'altra VLAN non arriverebbero (vedi sotto, **Nome**).",
            ]),
            .heading("Durante la scansione"),
            .text("Premi **Avvia scansione** (o ⌘R). I dispositivi compaiono nella tabella man mano che rispondono; sotto il pulsante vedi la fase in corso (*Ricerca dispositivi*, *Nomi e produttori*, *Porte*), quanti dispositivi sono attivi, le porte aperte trovate e il tempo trascorso. **Ferma** interrompe la scansione tenendo quanto trovato."),
            .heading("Come vengono riconosciuti i dispositivi"),
            .bullets([
                "**Presenza** — ping e tentativi di connessione su porte comuni: basta una risposta, anche un rifiuto, per sapere che il dispositivo è acceso.",
                "**Produttore** — dal MAC, con una tabella dei produttori inclusa nell'app. Se il MAC è *privato (casuale)*, come fanno telefoni e tablet, il produttore non si può sapere.",
                "**Nome** — da DNS inverso, Bonjour/mDNS (Mac, iPhone, stampanti, altoparlanti) e NetBIOS (Windows e Samba). mDNS e NetBIOS di solito non attraversano le VLAN: con il profilo **Approfondita** NetScout chiede il nome anche ai PC Windows (condivisione file, porta 445, e Desktop remoto, 3389), legge i certificati delle pagine web (443, 8443, 5001) e interroga direttamente i server DNS della rete scansionata e quelli indicati in **Impostazioni → Scansione** (ad esempio i domain controller).",
                "**Tipo** — dedotto da produttore, nome, porte aperte e servizi annunciati (router, computer, stampante, telecamera, NAS, TV, console, IoT…).",
            ]),
            .heading("Cambi di rete"),
            .text("NetScout segue la rete mentre è aperto: se ti colleghi a un'altra Wi‑Fi, attacchi un cavo o cambia l'indirizzo, l'elenco delle interfacce si aggiorna da solo e la barra in basso mostra «Rete cambiata». Se stavi usando una rete rilevata automaticamente che non c'è più, la destinazione passa alla nuova; una destinazione scritta a mano non viene toccata."),
        ]),
        HelpTopic(title: "Elenco dei dispositivi", symbol: "tablecells", blocks: [
            .text("La tabella mostra per ogni dispositivo **tipo, IP, nome, produttore, MAC, latenza e numero di porte aperte**. Clicca sull'intestazione di una colonna per ordinare."),
            .heading("Ordinare per più colonne"),
            .bullets([
                "**Clic** sull'intestazione — ordina solo per quella colonna; un altro clic inverte l'ordine.",
                "**⇧-clic** — aggiunge la colonna all'ordinamento dopo le altre; un altro ⇧-clic la inverte, un terzo la toglie.",
                "Le colonne che partecipano mostrano la priorità e il verso, per esempio **Produttore ¹▲** e **Nome ²▼**: prima per produttore, a parità di produttore per nome.",
            ]),
            .heading("Larghezza delle colonne"),
            .text("Fai **doppio clic sul separatore** a destra dell'intestazione di una colonna per adattarne la larghezza al testo più lungo, intestazione compresa, tra i dispositivi elencati."),
            .heading("Filtrare e cercare"),
            .bullets([
                "Nella sezione **Dispositivi**, in cima alla barra laterale, trovi quanti dispositivi ci sono per ciascun tipo: clicca un tipo per vedere solo quelli, ricliccalo (o scegli **Tutti**) per tornare all'elenco completo.",
                "Il campo di ricerca in alto a destra, sopra la scheda del dispositivo e largo quanto lei, filtra per IP, nome, produttore o MAC.",
            ]),
            .heading("Dispositivi spenti"),
            .text("Dopo un confronto con un profilo puoi aggiungere alla tabella i dispositivi salvati ma non trovati: compaiono in grigio, in fondo, con l'etichetta **Spento**. Dal menu contestuale (clic destro) puoi toglierli dall'elenco."),
        ]),
        HelpTopic(title: "Scheda del dispositivo", symbol: "sidebar.right", blocks: [
            .text("Seleziona un dispositivo per aprire la sua scheda a destra. Il pulsante con l'icona del pannello, in alto a destra dopo il campo di ricerca, la mostra o la nasconde."),
            .heading("Cosa contiene"),
            .bullets([
                "**Indirizzo IP, MAC, produttore e nomi** — tutti selezionabili per copiarli.",
                "**Latenza** — il tempo di risposta al ping; «Non risponde al ping» se il dispositivo ha risposto solo su qualche porta.",
                "**Porte aperte** — numero e servizio di ciascuna porta.",
            ]),
            .heading("Azioni sulle porte"),
            .bullets([
                "**Apri** — sulle porte web (80, 443, 8080, 8443, …) apre l'interfaccia del dispositivo nel browser.",
                "**Terminale** — sulle porte SSH e Telnet apre una sessione nel Terminale. Vedi *Connessioni remote*.",
                "**Merlin** — sulle porte SSH, Telnet e RDP, se l'integrazione è attiva. Vedi *Integrazioni*.",
            ]),
            .heading("Scansione approfondita"),
            .text("Il pulsante **Scansione approfondita**, nella scheda sotto il nome del dispositivo, rianalizza solo quel dispositivo con il profilo approfondito, per scoprire porte e servizi che la scansione rapida o standard non controlla. Su un dispositivo spento diventa **Riscansiona**, per verificare se nel frattempo si è acceso."),
        ]),
        HelpTopic(title: "Connessioni remote", symbol: "terminal", blocks: [
            .text("Accanto alle porte **SSH (22)** e **Telnet (23)** il pulsante **Terminale** apre una sessione nell'app Terminale di macOS."),
            .bullets([
                "Si apre una finestrella dove inserire **utente** e **password**. L'utente viene ricordato per ogni dispositivo.",
                "La password **non viene salvata**: serve solo per l'accesso e viene cancellata appena la sessione parte. Lasciala vuota per scriverla nel Terminale o per usare una chiave SSH.",
                "Al primo collegamento SSH la chiave del dispositivo viene accettata da sola; se in seguito cambia, il collegamento viene rifiutato per sicurezza.",
                "La finestra del Terminale si chiude quando la sessione finisce.",
            ]),
            .note("macOS non include più Telnet. Per usarlo installalo con Homebrew: `brew install telnet`."),
            .text("Le sessioni **RDP** (Desktop remoto, porta 3389) non si aprono nel Terminale: si possono aprire con Merlin."),
        ]),
        HelpTopic(title: "Accensione (Wake-on-LAN)", symbol: "power", blocks: [
            .text("Un dispositivo spento salvato in un profilo si può accendere a distanza con il **Wake-on-LAN**: seleziona il dispositivo spento e premi **Accendi (Wake-on-LAN)**."),
            .bullets([
                "NetScout invia il «pacchetto magico» al MAC del dispositivo; se il Wake-on-LAN è attivo, si accende in qualche secondo.",
                "Per verificare premi **Riscansiona**: se risponde, torna tra i dispositivi accesi.",
                "Serve il MAC: senza, il pulsante è disattivato.",
                "I dispositivi con MAC privato (telefoni, tablet) di solito non si accendono così.",
            ]),
            .note("Il Wake-on-LAN va abilitato sul dispositivo stesso (nel BIOS/UEFI o nelle impostazioni di risparmio energia) e funziona di norma solo sulla stessa rete."),
        ]),
        HelpTopic(title: "Profili e confronti", symbol: "archivebox", blocks: [
            .text("Un **profilo** è una fotografia della rete: l'elenco dei dispositivi trovati in una scansione, con le loro porte."),
            .heading("Salvare"),
            .text("A scansione finita, nella sezione **Risultato** premi **Salva come profilo…** e dai un nome. I profili si trovano nella scheda **Profili salvati**, dove puoi consultarli, rinominarli o eliminarli (clic destro)."),
            .heading("Confrontare"),
            .text("Usa **Confronta con profilo** nella barra laterale (o il pulsante nel profilo) per vedere cosa è cambiato rispetto a una scansione salvata:"),
            .bullets([
                "**Nuovi dispositivi** — presenti ora, assenti nel profilo.",
                "**Spenti o scomparsi** — nel profilo, ma non trovati ora.",
                "**Di nuovo accesi** — erano spenti quando hai salvato il profilo.",
                "**Stesso MAC, IP diverso** — lo stesso dispositivo ha cambiato indirizzo.",
                "**Stesso IP, MAC diverso** — all'indirizzo ora c'è un altro dispositivo.",
                "**Altre modifiche** — nome, produttore, tipo o porte aperte cambiati.",
            ]),
            .text("I dispositivi vengono riconosciuti prima dal MAC, poi dall'IP."),
            .heading("Dal confronto puoi"),
            .bullets([
                "**Aggiungere gli spenti alla scansione** — per vederli in tabella e accenderli con il Wake-on-LAN.",
                "**Aggiornare il profilo** — sostituirlo con la scansione attuale; i dispositivi spenti restano nel profilo.",
            ]),
            .heading("Note sui dispositivi"),
            .text("Nella tabella di un profilo, la colonna **Note** si modifica con un clic: scrivi dove si trova un dispositivo, a chi appartiene o qualsiasi altra informazione. Le note restano legate al dispositivo (dal MAC) anche quando aggiorni il profilo."),
            .text("La colonna **Note** c'è anche nella tabella della scansione: mostra le note del profilo della rete riconosciuta, o di quello in cui hai appena salvato la scansione. Passando col mouse su una riga compare una matita nella colonna: cliccala per scrivere. La stessa nota si modifica anche nella scheda del dispositivo selezionato, a destra. Le modifiche, su tutti i dispositivi, vanno nel profilo: si salvano con **Salva** (⌘S) nella sezione **Note** della barra laterale o nella scheda, e chiudendo la finestra NetScout chiede cosa fare di quelle non salvate. Se la rete non è in nessun profilo, salva prima la scansione come profilo. Salvando come nuovo profilo una scansione riconosciuta, le sue note passano anche al nuovo profilo."),
            .text("Le modifiche si salvano con **Salva note** (⌘S) o si scartano con **Annulla modifiche**; finché non le salvi il profilo ha un pallino arancione nell'elenco. Se chiudi la finestra o esci con note non salvate, NetScout chiede se salvarle."),
            .heading("Cercare"),
            .text("Il campo di ricerca in alto filtra i profili per nome o rete scansionata, oppure per i dispositivi che contengono (IP, nome, produttore, MAC o nota). Se a corrispondere sono solo alcuni dispositivi, la tabella del profilo mostra solo quelli."),
            .heading("Rete riconosciuta"),
            .text("Durante una scansione NetScout confronta i dispositivi trovati con i profili salvati e, se riconosce la rete, te lo segnala proponendo di confrontarla con il profilo."),
            .bullets([
                "Basta ritrovare il **router** (lo stesso MAC del gateway) in un profilo.",
                "Senza il router servono almeno **3 dispositivi** del profilo, e almeno la metà di quelli riconoscibili.",
                "Contano solo identificatori stabili: il MAC assegnato dal produttore e, se il MAC manca, l'identificativo UPnP. IP, nomi e MAC privati (telefoni, tablet) non vengono usati.",
                "Se due profili di reti diverse corrispondono allo stesso modo, NetScout non propone nulla.",
            ]),
            .heading("Esportare in CSV"),
            .text("Il pulsante **Esporta CSV** in alto (o il clic destro su un profilo) salva un file con un dispositivo per riga: profilo, rete, date, stato, IP, nomi, tipo, produttore, MAC, latenza, porte aperte, servizi e note. Il file usa il punto e virgola come separatore e si apre direttamente in Excel o Numeri con le lettere accentate corrette."),
        ]),
        HelpTopic(title: "Integrazioni", symbol: "puzzlepiece.extension", blocks: [
            .text("Le integrazioni si gestiscono in **NetScout → Impostazioni… → Integrazioni**."),
            .heading("Merlin"),
            .text("Merlin è un'app per sessioni remote SSH, Telnet e RDP. Con l'integrazione attiva, nella scheda del dispositivo compare il pulsante **Merlin** accanto a ogni porta SSH, Telnet o RDP: un clic apre la sessione in Merlin con protocollo e indirizzo già compilati (utente e password li chiede Merlin)."),
            .bullets([
                "L'integrazione è **disattivata** dopo l'installazione: attivala dalle Impostazioni se usi Merlin.",
                "Le Impostazioni mostrano se Merlin è installato e pronto.",
                "Serve una versione di Merlin che gestisca i collegamenti `merlin://` (dalla 0.3); con versioni precedenti il pulsante resta disattivato e spiega perché.",
            ]),
        ]),
        HelpTopic(title: "Aggiornamenti", symbol: "arrow.triangle.2.circlepath", blocks: [
            .text("NetScout si aggiorna dalle versioni pubblicate su GitHub. Le opzioni sono in **NetScout → Impostazioni… → Aggiornamento**."),
            .heading("Aggiornamenti automatici"),
            .text("Con l'opzione **Aggiornamenti automatici** attiva, NetScout controlla all'apertura e poi ogni 6 ore; quando trova una nuova versione la scarica, la installa e si riavvia da solo. Se è in corso una scansione aspetta che finisca."),
            .text("Con l'opzione disattivata (impostazione iniziale) il controllo avviene comunque, ma l'aggiornamento resta una tua scelta: nella barra in basso compare **Disponibile la versione …** con i pulsanti **Novità** e **Aggiorna e riavvia**."),
            .heading("Controllo manuale"),
            .text("Premi **Cerca aggiornamenti** nelle Impostazioni (o **NetScout → Controlla aggiornamenti…**). Nelle Impostazioni vedi anche la versione installata e l'ora dell'ultimo controllo."),
            .heading("Sicurezza"),
            .text("Prima di installare, NetScout verifica che il file scaricato contenga davvero NetScout, alla versione annunciata e con una firma valida; se qualcosa non torna l'aggiornamento viene annullato e l'app resta com'è."),
            .note("Per aggiornarsi da sola l'app deve trovarsi in una cartella in cui può scrivere, come Applicazioni."),
        ]),
        HelpTopic(title: "Scorciatoie", symbol: "keyboard", blocks: [
            .bullets([
                "**⌘R** — avvia o ferma la scansione",
                "**⌘,** — apri le Impostazioni",
                "**⌘?** — apri questa guida",
                "**⌘F** — cerca nell'elenco dei dispositivi",
                "**Invio** nel campo destinazione — avvia la scansione",
                "**⌘W** — chiudi la finestra",
            ]),
        ]),
    ]
}
