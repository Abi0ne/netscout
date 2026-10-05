// swift-tools-version: 5.10
//
// NetScout for macOS, built with SwiftPM (no Xcode project needed):
//
//   netscout_coreFFI  C module: the UniFFI header + modulemap
//   NetScoutCore      UniFFI Swift bindings, linked against the Rust
//                     static library (core → libnetscout_core.a)
//   NetScout          the SwiftUI app
//
// Build the Rust library first (scripts/build-app.sh does everything):
//
//   cargo build --release --target aarch64-apple-darwin
//   swift build -c release --package-path apple

import PackageDescription

/// The Rust static library, linked by full path so the linker never picks the
/// cdylib that cargo leaves next to it.
let rustLib = Context.packageDirectory
    + "/../target/aarch64-apple-darwin/release/libnetscout_core.a"

let package = Package(
    name: "NetScout",
    platforms: [.macOS(.v14)],
    targets: [
        .target(
            name: "netscout_coreFFI",
            path: "Sources/netscout_coreFFI"
        ),
        .target(
            name: "NetScoutCore",
            dependencies: ["netscout_coreFFI"],
            path: "Sources/NetScoutCore",
            linkerSettings: [
                .unsafeFlags([rustLib]),
                // Native libraries Rust's std needs (`--print native-static-libs`).
                .linkedLibrary("iconv"),
            ]
        ),
        .executableTarget(
            name: "NetScout",
            dependencies: ["NetScoutCore"],
            path: "Sources/NetScout"
        ),
    ]
)
