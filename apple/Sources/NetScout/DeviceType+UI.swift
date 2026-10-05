import NetScoutCore

extension DeviceType: CaseIterable {
    public static let allCases: [DeviceType] = [
        .router, .modem, .computer, .laptop, .server, .phone, .tablet, .printer,
        .camera, .nvr, .nasc, .smartTv, .gaming, .voip, .audio, .iot, .other, .unknown,
    ]

    /// User-facing name.
    var label: String {
        switch self {
        case .router: "Rete"
        case .modem: "Modem"
        case .computer: "Computer"
        case .laptop: "Portatile"
        case .server: "Server"
        case .phone: "Telefono"
        case .tablet: "Tablet"
        case .printer: "Stampante"
        case .camera: "Telecamera"
        case .nvr: "NVR"
        case .nasc: "NAS"
        case .smartTv: "TV"
        case .gaming: "Console"
        case .voip: "Telefono VoIP"
        case .audio: "Audio"
        case .iot: "IoT"
        case .other: "Altro"
        case .unknown: "Sconosciuto"
        }
    }

    /// SF Symbol name.
    var symbol: String {
        switch self {
        case .router: "wifi.router"
        case .modem: "network"
        case .computer: "desktopcomputer"
        case .laptop: "laptopcomputer"
        case .server: "server.rack"
        case .phone: "iphone"
        case .tablet: "ipad"
        case .printer: "printer"
        case .camera: "web.camera"
        case .nvr: "video"
        case .nasc: "externaldrive.connected.to.line.below"
        case .smartTv: "tv"
        case .gaming: "gamecontroller"
        case .voip: "phone"
        case .audio: "hifispeaker"
        case .iot: "sensor"
        case .other: "circle.dashed"
        case .unknown: "questionmark.circle"
        }
    }
}

extension ScanProfile {
    var label: String {
        switch self {
        case .quick: "Rapida"
        case .standard: "Standard"
        case .deep: "Approfondita"
        }
    }
}

extension ScanPhase {
    var label: String {
        switch self {
        case .probing: "Ricerca dispositivi"
        case .resolving: "Nomi e produttori"
        case .portScanning: "Porte"
        case .done: "Completata"
        }
    }
}

extension Host {
    /// Best display name: first hostname without the ".local" suffix.
    var displayName: String? {
        hostnames.first.map { $0.hasSuffix(".local") ? String($0.dropLast(6)) : $0 }
    }
}
