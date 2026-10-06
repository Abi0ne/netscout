import AppKit
import Foundation

/// A remote-session protocol a scanned port can be opened with.
enum RemoteProtocol: String {
    case ssh, telnet, rdp

    var label: String {
        switch self {
        case .ssh: "SSH"
        case .telnet: "Telnet"
        case .rdp: "Desktop remoto (RDP)"
        }
    }

    /// The protocol a port speaks, if it is one we can open a session to.
    init?(port: Port) {
        switch (port.service?.lowercased(), port.number) {
        case ("ssh", _), (_, 22): self = .ssh
        case ("telnet", _), (_, 23): self = .telnet
        case ("ms-wbt-server", _), ("rdp", _), (_, 3389): self = .rdp
        default: return nil
        }
    }
}

/// Opens a session in Merlin (bundle id `eu.raapp.imeterminal`) through its
/// URL scheme, passing only the protocol and the address — Merlin asks for
/// the user and password itself:
///
///     merlin://ssh/172.31.31.162
///     merlin://telnet/172.31.31.162
///     merlin://rdp/172.31.31.162
///
/// Merlin declares `merlin://` from 0.3; older versions are detected by
/// bundle id and reported as not handling links.
enum MerlinLauncher {
    static let bundleIdentifier = "eu.raapp.imeterminal"

    /// The user default behind Settings → Integrazioni → Merlin; off until
    /// the user turns it on.
    static let enabledKey = "integrations.merlin"

    /// Where Merlin is installed, if it is.
    static var appURL: URL? {
        NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleIdentifier)
    }

    static var isInstalled: Bool { appURL != nil }

    /// The installed version, e.g. "0.3.1".
    static var version: String? {
        appURL.flatMap { Bundle(url: $0)?.infoDictionary?["CFBundleShortVersionString"] as? String }
    }

    /// Some app (Merlin from 0.3) handles `merlin://`.
    static var handlesLinks: Bool {
        NSWorkspace.shared.urlForApplication(toOpen: URL(string: "merlin://ssh/127.0.0.1")!) != nil
    }

    /// Why a device cannot be opened in Merlin, if it cannot.
    static var unavailableReason: LaunchError? {
        if !isInstalled { return .notInstalled }
        if !handlesLinks { return .linksUnsupported(version: version) }
        return nil
    }

    enum LaunchError: LocalizedError {
        case notInstalled
        case linksUnsupported(version: String?)
        case invalidAddress

        var errorDescription: String? {
            switch self {
            case .notInstalled:
                "Merlin non installato."
            case .linksUnsupported(let version):
                "Merlin\(version.map { " \($0)" } ?? "") è installato ma non gestisce i collegamenti merlin://. Serve una versione di Merlin che li supporti."
            case .invalidAddress:
                "Indirizzo non valido per Merlin."
            }
        }
    }

    static func url(_ kind: RemoteProtocol, ip: String) -> URL? {
        URL(string: "merlin://\(kind.rawValue)/\(ip)")
    }

    static func open(_ kind: RemoteProtocol, ip: String) throws {
        if let reason = unavailableReason { throw reason }
        guard let url = url(kind, ip: ip) else { throw LaunchError.invalidAddress }
        NSWorkspace.shared.open(url)
    }
}
