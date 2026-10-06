import SwiftUI

/// NetScout → Impostazioni… (⌘,): integrations and updates.
struct SettingsView: View {
    var body: some View {
        TabView {
            IntegrationsSettings()
                .tabItem { Label("Integrazioni", systemImage: "puzzlepiece.extension") }
            UpdateSettings()
                .tabItem { Label("Aggiornamento", systemImage: "arrow.triangle.2.circlepath") }
        }
        .frame(width: 520)
    }
}

private struct IntegrationsSettings: View {
    @AppStorage(MerlinLauncher.enabledKey) private var merlinEnabled = false

    var body: some View {
        Form {
            Section {
                Toggle(isOn: $merlinEnabled) {
                    Text("Integrazione con Merlin")
                    Text("Aggiunge il pulsante «Merlin» accanto alle porte SSH, Telnet e RDP nella scheda del dispositivo, per aprire la sessione in Merlin.")
                }
                LabeledContent("Stato") {
                    Label(status.text, systemImage: status.symbol)
                        .foregroundStyle(status.ok ? Color.green : Color.orange)
                }
            } header: {
                Text("Merlin")
            }
        }
        .formStyle(.grouped)
    }

    private var status: (text: String, symbol: String, ok: Bool) {
        if let reason = MerlinLauncher.unavailableReason {
            return (reason.localizedDescription, "exclamationmark.triangle.fill", false)
        }
        return ("Merlin \(MerlinLauncher.version ?? "") installato e pronto", "checkmark.circle.fill", true)
    }
}

private struct UpdateSettings: View {
    @Environment(Updater.self) private var updater

    var body: some View {
        Form {
            Section {
                Toggle(isOn: Binding(get: { updater.automatic }, set: { updater.setAutomatic($0) })) {
                    Text("Aggiornamenti automatici")
                    Text("Quando su GitHub esce una nuova versione, NetScout la scarica, la verifica e si riavvia aggiornato (dopo aver finito la scansione in corso). Il controllo avviene all'apertura e poi ogni 6 ore.")
                }
                .disabled(!Updater.canInstall)
                if !Updater.canInstall {
                    Text("Da questa posizione l'app non può sostituirsi da sola: spostala in Applicazioni per usare gli aggiornamenti.")
                        .font(.caption)
                        .foregroundStyle(.orange)
                }
            }
            Section {
                LabeledContent("Versione installata", value: Updater.currentVersion)
                LabeledContent("Ultimo controllo") {
                    Text(updater.lastCheck?.formatted(date: .abbreviated, time: .shortened) ?? "—")
                }
                LabeledContent("Stato") { statusView }
                HStack {
                    Spacer()
                    if case .available(let release) = updater.state {
                        Link("Novità", destination: release.page)
                        Button("Aggiorna e riavvia") { Task { await updater.install(release) } }
                            .buttonStyle(.borderedProminent)
                    }
                    Button("Cerca aggiornamenti") { Task { await updater.check() } }
                        .disabled(updater.state == .checking || updater.state == .downloading)
                }
            }
        }
        .formStyle(.grouped)
    }

    @ViewBuilder
    private var statusView: some View {
        switch updater.state {
        case .idle:
            Text("—")
        case .checking:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Controllo in corso…")
            }
        case .upToDate:
            Label("Hai l'ultima versione", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
        case .available(let release):
            Label("Disponibile la versione \(release.version)", systemImage: "arrow.down.circle.fill")
                .foregroundStyle(.tint)
        case .downloading:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Scarico l'aggiornamento…")
            }
        case .failed(let message):
            Label(message, systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
