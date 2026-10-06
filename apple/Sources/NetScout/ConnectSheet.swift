import SwiftUI

/// A remote session the user asked to open.
struct ConnectRequest: Identifiable {
    let kind: RemoteProtocol
    let host: String
    let port: UInt16
    var id: String { "\(host):\(port)" }
}

/// Asks with which user and password to open the session in Terminal, then
/// opens it. The user name is remembered per host; the password is never
/// stored.
struct ConnectSheet: View {
    @Environment(\.dismiss) private var dismiss
    let request: ConnectRequest
    @ViewState private var user = ""
    @ViewState private var password = ""
    @ViewState private var failure: String?

    private var userKey: String { "terminal.user.\(request.host)" }

    /// Why the session cannot be opened, if it cannot.
    private var blocker: String? {
        request.kind == .telnet && TerminalLauncher.telnetPath == nil
            ? TerminalLauncher.LaunchError.telnetMissing.localizedDescription : nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("\(request.kind.label) nel Terminale").font(.title3.weight(.semibold))
                Text("\(request.host) · porta \(request.port)")
                    .font(.system(.body, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            Form {
                TextField("Utente", text: $user)
                SecureField("Password", text: $password)
            }
            Text("Lascia vuota la password per inserirla nel Terminale (o per usare una chiave SSH). La password non viene salvata.")
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
        }
    }

    private func connect() {
        let user = user.trimmingCharacters(in: .whitespaces)
        do {
            try TerminalLauncher.open(request.kind, host: request.host, port: request.port,
                                      user: user, password: password)
            UserDefaults.standard.set(user, forKey: userKey)
            dismiss()
        } catch {
            failure = error.localizedDescription
        }
    }
}
