import SwiftUI

/// A remote session the user asked to open.
struct ConnectRequest: Identifiable {
    let kind: RemoteProtocol
    let host: String
    let port: UInt16
    var id: String { "\(host):\(port)" }
}

/// Where a session opens.
enum ConnectApp: String, CaseIterable {
    case terminal, merlin

    var label: String {
        switch self {
        case .terminal: "Terminale"
        case .merlin: "Merlin"
        }
    }
}

/// Asks where to open the session and with which user (and, in Terminal,
/// password), then opens it. The user name and the app are remembered per
/// host; the password is never stored.
struct ConnectSheet: View {
    @Environment(\.dismiss) private var dismiss
    let request: ConnectRequest
    @ViewState private var app: ConnectApp = .terminal
    @ViewState private var user = ""
    @ViewState private var password = ""
    @ViewState private var failure: String?

    private var userKey: String { "terminal.user.\(request.host)" }
    private static let appKey = "connect.app"

    /// The apps that can open this protocol (Terminal cannot do RDP).
    private var apps: [ConnectApp] {
        TerminalLauncher.supports(request.kind) ? ConnectApp.allCases : [.merlin]
    }

    /// Why the session cannot be opened as configured, if it cannot.
    private var blocker: String? {
        switch app {
        case .merlin:
            MerlinLauncher.unavailableReason?.localizedDescription
        case .terminal:
            request.kind == .telnet && TerminalLauncher.telnetPath == nil
                ? TerminalLauncher.LaunchError.telnetMissing.localizedDescription : nil
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Connetti con \(request.kind.label)").font(.title3.weight(.semibold))
                Text("\(request.host) · porta \(request.port)")
                    .font(.system(.body, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            Form {
                Picker("Apri con", selection: $app) {
                    ForEach(apps, id: \.self) { Text($0.label).tag($0) }
                }
                .pickerStyle(.segmented)
                TextField("Utente", text: $user)
                if app == .terminal {
                    SecureField("Password", text: $password)
                }
            }
            Text(app == .terminal
                ? "Lascia vuota la password per inserirla nel Terminale (o per usare una chiave SSH). La password non viene salvata."
                : "La password la chiede Merlin.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if let message = failure ?? blocker {
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.orange)
                    .textSelection(.enabled)
            }
            HStack {
                Spacer()
                Button("Annulla", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Connetti") { connect() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(blocker != nil)
            }
        }
        .padding(20)
        .frame(width: 420)
        .onAppear {
            user = UserDefaults.standard.string(forKey: userKey) ?? ""
            let saved = UserDefaults.standard.string(forKey: Self.appKey).flatMap(ConnectApp.init)
            app = saved.flatMap { apps.contains($0) ? $0 : nil } ?? apps[0]
        }
        .onChange(of: app) { failure = nil }
    }

    private func connect() {
        let user = user.trimmingCharacters(in: .whitespaces)
        do {
            switch app {
            case .terminal:
                try TerminalLauncher.open(request.kind, host: request.host, port: request.port,
                                          user: user, password: password)
            case .merlin:
                try MerlinLauncher.open(request.kind, host: request.host, port: request.port, user: user)
            }
            UserDefaults.standard.set(user, forKey: userKey)
            UserDefaults.standard.set(app.rawValue, forKey: Self.appKey)
            dismiss()
        } catch {
            failure = error.localizedDescription
        }
    }
}
