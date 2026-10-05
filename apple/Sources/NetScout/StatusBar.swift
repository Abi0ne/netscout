import SwiftUI

/// The strip at the bottom of the window: the app version and, when there is
/// one, the update on offer.
struct StatusBar: View {
    @Environment(Updater.self) private var updater

    var body: some View {
        HStack(spacing: 10) {
            Text("NetScout \(Updater.currentVersion)")
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
            Spacer()
            updateStatus
        }
        .padding(.horizontal, 12)
        .frame(height: 26)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }

    @ViewBuilder
    private var updateStatus: some View {
        switch updater.state {
        case .idle:
            EmptyView()
        case .checking:
            HStack(spacing: 6) {
                ProgressView().controlSize(.mini)
                Text("Controllo aggiornamenti…").font(.caption).foregroundStyle(.secondary)
            }
        case .upToDate:
            Label("Aggiornata", systemImage: "checkmark.circle")
                .font(.caption)
                .foregroundStyle(.secondary)
        case .available(let release):
            HStack(spacing: 8) {
                Label("Disponibile la versione \(release.version)", systemImage: "arrow.down.circle.fill")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.tint)
                Link("Novità", destination: release.page)
                    .font(.caption)
                Button("Aggiorna e riavvia") {
                    Task { await updater.install(release) }
                }
                .controlSize(.small)
                .buttonStyle(.borderedProminent)
            }
        case .downloading:
            HStack(spacing: 6) {
                ProgressView().controlSize(.mini)
                Text("Scarico l'aggiornamento…").font(.caption).foregroundStyle(.secondary)
            }
        case .failed(let message):
            HStack(spacing: 8) {
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .lineLimit(1)
                    .help(message)
                Button("Riprova") { Task { await updater.check() } }
                    .controlSize(.small)
            }
        }
    }
}
