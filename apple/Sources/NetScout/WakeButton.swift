import NetScoutCore
import SwiftUI

/// Sends a Wake-on-LAN packet to a device that is off, and says what happened.
struct WakeButton: View {
    @Environment(ScanModel.self) private var model
    let host: Host
    /// Compact form for lists (icon button + short status).
    var compact = false
    @ViewState private var sentAt: Date?
    @ViewState private var failure: String?

    var body: some View {
        HStack(spacing: 8) {
            Button {
                failure = model.wake(host)
                sentAt = failure == nil ? Date() : nil
            } label: {
                Label(compact ? "Accendi" : "Accendi (Wake-on-LAN)", systemImage: "power")
            }
            .controlSize(compact ? .small : .regular)
            .disabled(host.mac == nil)
            .help(help)
            if let failure {
                Text(failure).font(.caption).foregroundStyle(.red).lineLimit(compact ? 1 : nil)
            } else if let sentAt {
                Text(compact
                    ? "Inviato alle \(sentAt.formatted(date: .omitted, time: .shortened))"
                    : "Pacchetto inviato alle \(sentAt.formatted(date: .omitted, time: .standard)). Se il dispositivo ha il Wake-on-LAN attivo si accende in qualche secondo: riscansionalo per verificare.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: !compact)
            }
        }
    }

    private var help: String {
        guard let mac = host.mac else { return "MAC sconosciuto: il Wake-on-LAN non è possibile." }
        if host.hasPrivateMAC {
            return "MAC privato (casuale): questo dispositivo probabilmente non si accende con il Wake-on-LAN."
        }
        return "Invia il pacchetto Wake-on-LAN a \(mac)"
    }
}

extension Host {
    /// Locally administered MAC: phones and tablets use random ones per network.
    var hasPrivateMAC: Bool {
        guard let mac, let first = UInt8(mac.prefix(2), radix: 16) else { return false }
        return first & 0x02 != 0
    }
}
