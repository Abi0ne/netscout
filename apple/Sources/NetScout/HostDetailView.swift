import AppKit
import NetScoutCore
import SwiftUI

struct HostDetailView: View {
    @Environment(ScanModel.self) private var model
    let host: Host

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                header
                infoGrid
                portsSection
            }
            .padding(24)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .toolbar {
            ToolbarItem {
                Button {
                    model.deepScan(ip: host.ip)
                } label: {
                    if model.deepScanning.contains(host.ip) {
                        ProgressView().controlSize(.small)
                    } else {
                        Label("Scansione approfondita", systemImage: "scope")
                    }
                }
                .help("Riscansiona questo dispositivo con il profilo approfondito")
                .disabled(model.deepScanning.contains(host.ip))
            }
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
        }
    }

    private var infoGrid: some View {
        Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 8) {
            row("Indirizzo IP", host.ip, monospaced: true)
            row("MAC", host.mac ?? "—", monospaced: true)
            row("Produttore", host.vendor ?? macNote)
            row("Nomi", host.hostnames.isEmpty ? "—" : host.hostnames.joined(separator: "\n"))
            row("Latenza", host.rttMs.map { String(format: "%.2f ms", $0) } ?? "Non risponde al ping")
            row("Visto alle", Date(timeIntervalSince1970: Double(host.lastSeen) / 1000)
                .formatted(date: .omitted, time: .standard))
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
            Text("Porte aperte").font(.headline)
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
                    }
                }
            }
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
