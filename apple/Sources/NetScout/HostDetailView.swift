import AppKit
import NetScoutCore
import SwiftUI

struct HostDetailView: View {
    @Environment(ScanModel.self) private var model
    let host: Host
    /// Known from a profile but not found by this scan.
    var offline = false
    @ViewState private var connecting: ConnectRequest?
    @ViewState private var merlinFailure: String?
    @AppStorage(MerlinLauncher.enabledKey) private var merlinEnabled = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                header
                if offline { wakeSection }
                noteSection
                deepScanSection
                infoGrid
                portsSection
            }
            .padding(24)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .sheet(item: $connecting) { request in
            ConnectSheet(request: request)
        }
    }

    /// Re-scans this device alone, with what the button does spelled out.
    private var deepScanSection: some View {
        let running = model.deepScanning.contains(host.ip)
        return VStack(alignment: .leading, spacing: 6) {
            Button {
                model.deepScan(ip: host.ip)
            } label: {
                if running {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Scansione in corso…")
                    }
                } else {
                    Label(offline ? "Riscansiona" : "Scansione approfondita", systemImage: "scope")
                }
            }
            .controlSize(.large)
            .disabled(running)
            Text(offline
                ? "Cerca di nuovo questo indirizzo per vedere se il dispositivo si è acceso."
                : "Riscansiona solo questo dispositivo con il profilo approfondito: più porte e più tempo per rispondere, per scoprire servizi sfuggiti alla scansione della rete.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var header: some View {
        HStack(spacing: 14) {
            Image(systemName: host.deviceType.symbol)
                .font(.system(size: 34))
                .foregroundStyle(.tint)
                .frame(width: 52, height: 52)
                .background(.tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 12))
            VStack(alignment: .leading, spacing: 2) {
                Text(host.displayName ?? host.ip)
                    .font(.title2.weight(.semibold))
                    .textSelection(.enabled)
                Text([host.deviceType.label, host.vendor].compactMap { $0 }.joined(separator: " · "))
                    .foregroundStyle(.secondary)
            }
            if offline {
                Label("Spento", systemImage: "power")
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(.orange)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .background(.orange.opacity(0.12), in: Capsule())
            }
        }
    }

    /// The device's note, in the profile the scan belongs to; the same note
    /// as in the table's column.
    @ViewBuilder
    private var noteSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Nota").font(.headline)
            if let id = model.scanNotesProfileID {
                TextField("Nota", text: Binding(
                    get: { model.note(profile: id, host: host) },
                    set: { model.setNote($0, profile: id, host: host) }
                ), prompt: Text("Scrivi una nota su questo dispositivo"), axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .lineLimit(2...6)
                if model.hasUnsavedNotes(profile: id) {
                    UnsavedNotesBar(profile: id)
                } else if let name = model.profiles.first(where: { $0.id == id })?.name {
                    Text(model.note(profile: id, host: host).isEmpty
                        ? "Andrà nel profilo «\(name)»."
                        : "Salvata nel profilo «\(name)».")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            } else {
                Text(model.hasFinishedScan
                    ? "Per scrivere note, salva la scansione come profilo (barra laterale, «Salva come profilo…»)."
                    : "Le note si potranno scrivere a scansione finita, salvandola come profilo.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var wakeSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Non trovato in questa scansione: è spento o non è più in rete.")
                .foregroundStyle(.secondary)
            WakeButton(host: host)
        }
    }

    private var infoGrid: some View {
        Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 8) {
            row("Indirizzo IP", host.ip, monospaced: true)
            row("MAC", host.mac ?? "—", monospaced: true)
            row("Produttore", host.vendor ?? macNote)
            row("Nomi", host.hostnames.isEmpty ? "—" : host.hostnames.joined(separator: "\n"))
            if offline {
                row("Visto l'ultima volta", Date(timeIntervalSince1970: Double(host.lastSeen) / 1000)
                    .formatted(date: .abbreviated, time: .shortened))
            } else {
                row("Latenza", host.rttMs.map { String(format: "%.2f ms", $0) } ?? "Non risponde al ping")
                row("Visto alle", Date(timeIntervalSince1970: Double(host.lastSeen) / 1000)
                    .formatted(date: .omitted, time: .standard))
            }
        }
    }

    /// Why a MAC has no vendor, when we can tell.
    private var macNote: String {
        guard let mac = host.mac, let first = UInt8(mac.prefix(2), radix: 16) else { return "—" }
        return first & 0x02 != 0 ? "MAC privato (casuale)" : "Non registrato"
    }

    @ViewBuilder
    private var portsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(offline ? "Porte aperte (all'ultimo rilevamento)" : "Porte aperte").font(.headline)
            if merlinEnabled, let merlinFailure {
                Label(merlinFailure, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.orange)
            }
            if host.openPorts.isEmpty {
                Text("Nessuna tra quelle verificate.").foregroundStyle(.secondary)
            } else {
                ForEach(host.openPorts, id: \.number) { port in
                    HStack {
                        Text("\(port.number)")
                            .font(.system(.body, design: .monospaced))
                            .frame(width: 60, alignment: .trailing)
                        Text(port.service ?? "—").foregroundStyle(.secondary)
                        Spacer()
                        if let url = webURL(port.number) {
                            Button("Apri") { NSWorkspace.shared.open(url) }
                                .buttonStyle(.link)
                        }
                        if !offline, let kind = RemoteProtocol(port: port) {
                            if TerminalLauncher.supports(kind) {
                                Button("Terminale") {
                                    connecting = ConnectRequest(kind: kind, host: host.ip, port: port.number)
                                }
                                .buttonStyle(.link)
                                .help("Apri una sessione \(kind.label) nel Terminale")
                            }
                            if merlinEnabled {
                                Button("Merlin") { openInMerlin(kind) }
                                    .buttonStyle(.link)
                                    .disabled(MerlinLauncher.unavailableReason != nil)
                                    .help(MerlinLauncher.unavailableReason?.localizedDescription ?? "Apri una sessione \(kind.label) in Merlin")
                            }
                        }
                    }
                }
            }
        }
    }

    private func openInMerlin(_ kind: RemoteProtocol) {
        do {
            try MerlinLauncher.open(kind, ip: host.ip)
            merlinFailure = nil
        } catch {
            merlinFailure = error.localizedDescription
        }
    }

    private func webURL(_ port: UInt16) -> URL? {
        switch port {
        case 80, 8080, 8000, 8008, 8888, 3000: URL(string: "http://\(host.ip):\(port)")
        case 443, 8443: URL(string: "https://\(host.ip):\(port)")
        default: nil
        }
    }

    private func row(_ title: String, _ value: String, monospaced: Bool = false) -> some View {
        GridRow(alignment: .firstTextBaseline) {
            Text(title).foregroundStyle(.secondary)
            Text(value)
                .font(monospaced ? .system(.body, design: .monospaced) : .body)
                .textSelection(.enabled)
        }
    }
}
