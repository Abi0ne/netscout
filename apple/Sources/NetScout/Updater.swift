import AppKit
import Foundation
import Observation

/// Self-update from the GitHub releases of the repo.
///
/// A release is the tag `v<version>` with the app attached as `NetScout.zip`
/// (scripts/release.sh publishes both). The app checks the latest release at
/// launch; installing downloads the zip, checks that it holds NetScout at the
/// announced version with a valid signature, and swaps the bundle once the
/// app has quit, then relaunches it.
@MainActor
@Observable
final class Updater {
    static let repository = "Abi0ne/netscout"
    static let assetName = "NetScout.zip"

    enum State: Equatable {
        case idle
        case checking
        case upToDate
        case available(Release)
        case downloading
        case failed(String)
    }

    struct Release: Equatable {
        let version: String
        let notes: String
        let page: URL
        let asset: URL
    }

    private(set) var state: State = .idle

    /// The running version (Info.plist), or the workspace version when the
    /// app runs outside its bundle (`swift run`).
    static var currentVersion: String {
        Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0"
    }

    /// Updates replace the bundle, so they need one (not `swift run`).
    static var canInstall: Bool {
        Bundle.main.bundleURL.pathExtension == "app"
            && FileManager.default.isWritableFile(atPath: Bundle.main.bundleURL.deletingLastPathComponent().path)
    }

    /// Ask GitHub for the latest release. `quiet` keeps errors out of the UI
    /// (the automatic check at launch, possibly offline).
    func check(quiet: Bool = false) async {
        guard state != .checking, state != .downloading else { return }
        state = .checking
        do {
            let release = try await Self.latestRelease()
            if let release, Self.isNewer(release.version, than: Self.currentVersion) {
                state = .available(release)
            } else {
                state = .upToDate
            }
        } catch {
            debugLog("update check failed: \(error)")
            state = quiet ? .idle : .failed("Controllo aggiornamenti non riuscito: \(error.localizedDescription)")
        }
    }

    /// Download `release`, verify it, and replace this app with it.
    func install(_ release: Release) async {
        guard Self.canInstall else {
            state = .failed("Aggiornamento automatico non possibile da qui: scarica la versione \(release.version) da GitHub.")
            return
        }
        state = .downloading
        do {
            let newApp = try await Self.download(release)
            try Self.replaceAndRelaunch(with: newApp)
        } catch {
            state = .failed("Aggiornamento non riuscito: \(error.localizedDescription)")
        }
    }

    // MARK: - GitHub

    private struct GitHubRelease: Decodable {
        struct Asset: Decodable {
            let name: String
            let browser_download_url: URL
        }
        let tag_name: String
        let body: String?
        let html_url: URL
        let draft: Bool
        let prerelease: Bool
        let assets: [Asset]
    }

    /// The latest published release that carries the app, if any.
    private static func latestRelease() async throws -> Release? {
        var request = URLRequest(url: URL(string: "https://api.github.com/repos/\(repository)/releases/latest")!)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("NetScout/\(currentVersion)", forHTTPHeaderField: "User-Agent")
        request.timeoutInterval = 15
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if status == 404 { return nil } // no release yet
        guard status == 200 else { throw UpdateError.http(status) }
        let gh = try JSONDecoder().decode(GitHubRelease.self, from: data)
        guard !gh.draft, !gh.prerelease,
              let asset = gh.assets.first(where: { $0.name == assetName }) else { return nil }
        let version = gh.tag_name.hasPrefix("v") ? String(gh.tag_name.dropFirst()) : gh.tag_name
        return Release(version: version, notes: gh.body ?? "", page: gh.html_url, asset: asset.browser_download_url)
    }

    /// "0.10.0" > "0.9.1"; missing components count as 0.
    static func isNewer(_ candidate: String, than current: String) -> Bool {
        let parse = { (v: String) in v.split(separator: ".").map { Int($0.prefix { $0.isNumber }) ?? 0 } }
        let (a, b) = (parse(candidate), parse(current))
        for i in 0..<max(a.count, b.count) {
            let (x, y) = (i < a.count ? a[i] : 0, i < b.count ? b[i] : 0)
            if x != y { return x > y }
        }
        return false
    }

    // MARK: - Install

    enum UpdateError: LocalizedError {
        case http(Int)
        case unpack
        case notNetScout
        case wrongVersion(String?)
        case badSignature

        var errorDescription: String? {
            switch self {
            case .http(let status): "GitHub ha risposto \(status)."
            case .unpack: "impossibile estrarre l'archivio scaricato."
            case .notNetScout: "l'archivio non contiene NetScout."
            case .wrongVersion(let v): "l'archivio contiene la versione \(v ?? "?"), non quella annunciata."
            case .badSignature: "la firma dell'app scaricata non è valida."
            }
        }
    }

    /// Download and unpack the release; returns the verified NetScout.app.
    private nonisolated static func download(_ release: Release) async throws -> URL {
        let (zip, response) = try await URLSession.shared.download(from: release.asset)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard status == 200 else { throw UpdateError.http(status) }

        let work = FileManager.default.temporaryDirectory
            .appending(path: "NetScout-update-\(UUID().uuidString)", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
        guard try run("/usr/bin/ditto", ["-x", "-k", zip.path, work.path]) == 0 else { throw UpdateError.unpack }
        try? FileManager.default.removeItem(at: zip)

        let app = work.appending(path: "NetScout.app", directoryHint: .isDirectory)
        guard let info = Bundle(url: app)?.infoDictionary,
              info["CFBundleIdentifier"] as? String == Bundle.main.bundleIdentifier ?? "dev.netscout.NetScout"
        else { throw UpdateError.notNetScout }
        let version = info["CFBundleShortVersionString"] as? String
        guard version == release.version else { throw UpdateError.wrongVersion(version) }
        guard try run("/usr/bin/codesign", ["--verify", "--deep", "--strict", app.path]) == 0 else {
            throw UpdateError.badSignature
        }
        return app
    }

    /// Hand the swap to a shell that waits for this process to exit, moves the
    /// old bundle aside, puts the new one in place (restoring the old one if
    /// that fails) and relaunches; then quit.
    private static func replaceAndRelaunch(with newApp: URL) throws {
        let current = Bundle.main.bundleURL.path
        let script = """
            while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
            old="$2.old-$$"
            mv "$2" "$old" || exit 1
            if mv "$3" "$2"; then rm -rf "$old"; else mv "$old" "$2"; fi
            open "$2"
            """
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", script, "netscout-update", String(getpid()), current, newApp.path]
        try process.run()
        NSApp.terminate(nil)
    }

    @discardableResult
    private nonisolated static func run(_ tool: String, _ arguments: [String]) throws -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        return process.terminationStatus
    }
}
