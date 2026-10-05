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

    var defaultPort: UInt16 {
        switch self {
        case .ssh: 22
        case .telnet: 23
        case .rdp: 3389
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

/// Opens sessions in Merlin (bundle id `eu.raapp.imeterminal`) through its
/// URL scheme:
///
///     merlin://ssh/192.168.1.10
///     merlin://ssh/andrea@192.168.1.10:2222
///     merlin://rdp/administrator@172.31.31.162
///     merlin://telnet/10.0.0.5
///
/// The scheme carries no password: Merlin asks for it. Merlin 0.2.0 does not
/// declare `merlin://` (its Info.plist has no CFBundleURLTypes), so the app
/// is detected by bundle id and the scheme separately.
enum MerlinLauncher {
    static let bundleIdentifier = "eu.raapp.imeterminal"

    /// Where Merlin is installed, if it is.
    static var appURL: URL? {
        NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleIdentifier)
    }

    static var isInstalled: Bool { appURL != nil }

    /// The installed version, e.g. "0.2.0".
    static var version: String? {
        appURL.flatMap { Bundle(url: $0)?.infoDictionary?["CFBundleShortVersionString"] as? String }
    }

    /// Some app (Merlin, once it declares the scheme) handles `merlin://`.
    static var handlesLinks: Bool {
        NSWorkspace.shared.urlForApplication(toOpen: URL(string: "merlin://ssh/127.0.0.1")!) != nil
    }

    /// Why a session cannot be opened in Merlin, if it cannot.
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

    /// `merlin://<protocol>/[user@]host[:port]`; the port only when it is not
    /// the protocol's default.
    static func url(_ kind: RemoteProtocol, host: String, port: UInt16, user: String) -> URL? {
        var target = host
        if !user.isEmpty {
            guard let encoded = user.addingPercentEncoding(withAllowedCharacters: .urlUserAllowed) else { return nil }
            target = "\(encoded)@\(host)"
        }
        if port != kind.defaultPort { target += ":\(port)" }
        return URL(string: "merlin://\(kind.rawValue)/\(target)")
    }

    static func open(_ kind: RemoteProtocol, host: String, port: UInt16, user: String) throws {
        if let reason = unavailableReason { throw reason }
        guard let url = url(kind, host: host, port: port, user: user) else { throw LaunchError.invalidAddress }
        NSWorkspace.shared.open(url)
    }
}
