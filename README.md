# NetScout

A fast, **non-privileged** LAN network scanner. The scanning engine is written
once in Rust (`core/`) and shared verbatim across platforms over UniFFI; each
platform is a thin UI (Swift/SwiftUI for macOS, GTK4/libadwaita for Linux,
Kotlin for Android later).

```
core/     netscout-core — tokio async engine, exposed via UniFFI
apple/    SwiftUI macOS 14+ app (MVVM, @Observable, Swift concurrency)
linux/    GTK4 + libadwaita app in Rust, linking the core directly
android/  (placeholder) Kotlin UI reusing the same engine as an AAR
scripts/  bindgen, app bundle and data build automation
```

## Download

Every [release](https://github.com/Abi0ne/netscout/releases/latest) carries:

- **macOS** (Apple Silicon, macOS 14+) — `NetScout-<version>.dmg`: open it
  and drag NetScout into Applications.
- **Linux** (x86-64, GTK 4.16+ and libadwaita 1.7+: Ubuntu 25.04+, Fedora
  42+, Debian 13+) — `netscout_<version>_amd64.deb`:

  ```
  sudo apt install ./netscout_<version>_amd64.deb
  ```

  apt installs the missing GTK and libadwaita packages; to update, install
  the newer `.deb` the same way.

## Layout

| Path | What |
| --- | --- |
| `core/` | Rust library crate `netscout-core`. Owns its own tokio runtime; streams results through the `ScanObserver` callback interface; cancellable; no root. |
| `core/src/platform/` | `trait PlatformNet` + per-OS impls selected by `cfg(target_os)`. |
| `apple/` | SwiftUI macOS app (SwiftPM, no Xcode project). Links `core` as an arm64 static library. See `apple/README.md`. |
| `scripts/` | `generate-bindings.sh`, `build-app.sh`, `build-deb.sh`, `release.sh`, `make-icon.sh`, `update-oui.sh` (refreshes `core/data/oui.tsv`, the embedded MAC-vendor table). |
| `linux/` | GTK4 / libadwaita app (separate Cargo project, not a workspace member, so the core builds where GTK is missing). Links `core` as a Rust crate. See `linux/README.md`. |
| `android/` | Placeholder for the later Kotlin port. |

## Public core API (UniFFI surface)

- **`Scanner`** — `new`, `detect_networks`, `set_network_info`, `start_scan`, `cancel`, `scan_host`.
- **`ProfileStore`** (`open_profile_store(dir)`) — saved scans as JSON files: `list`, `load`, `save`, `rename`, `delete`.
- **`diff_hosts(baseline, current)`** — compares a scan with a saved profile → `ScanDiff` (added / removed / changed / unchanged; devices matched by MAC, then IP).
- **`ScanObserver`** — async callback interface: `on_host`, `on_progress`, `on_finished`, `on_error`.
- **`ScanError`** — FFI error enum (thiserror + `uniffi::Error`).
- **Records** — `SavedProfile`, `ProfileSummary`, `ScanDiff`, `HostChange`, `Host`, `Port`, `ServiceInfo`, `SsdpInfo`, `NetworkInfo`, `ScanConfig`, `Progress`, `Summary`, `DeviceTypeCount`.
- **Enums** — `DeviceType`, `Transport`, `PortState`, `ScanProfile`, `ScanPhase`.

## Building the core (any host, for verification)

```
cargo build
cargo clippy -- -D warnings
cargo test
```

## Trying a scan from the terminal

```
cargo run --release --example scan                    # first detected LAN
cargo run --release --example scan -- 192.168.1.0/24  # explicit targets
cargo run --release --example scan -- --deep          # --quick | --standard | --deep
```

## Building the macOS app

```
scripts/generate-bindings.sh   # refresh the UniFFI bindings in apple/Sources
scripts/build-app.sh           # build apple/build/NetScout.app (arm64)
```

## Building the Linux app

On Linux (GTK 4.16+ and libadwaita 1.7+, e.g. Ubuntu 25.04+ or Fedora 42+):

```
sudo apt install libgtk-4-dev libadwaita-1-dev   # build dependencies
cargo run --manifest-path linux/Cargo.toml       # add `-- --scan` to scan at launch
scripts/build-deb.sh                             # dist/netscout_<version>_<arch>.deb
```

## Versions and updates

The version lives in `Cargo.toml` (`[workspace.package] version`); the app
bundle, the window's status bar and the release tag all take it from there.

The app checks the latest GitHub release at launch (and from *NetScout →
Controlla aggiornamenti…*, then every 6 hours); when it is newer it offers to
download it, verifies it and replaces itself. With *Impostazioni →
Aggiornamento → Aggiornamenti automatici* on (off by default) it installs
without asking, once no scan is running. To publish a version:

```
# bump version in Cargo.toml, commit, then:
scripts/release.sh [notes.md]  # build, zip, tag v<version>, push, gh release
```

Each release carries `NetScout.zip` (for the updater),
`NetScout-<version>.dmg` (for new installs, `scripts/make-dmg.sh`) and the
Linux `netscout_<version>_amd64.deb`, which `scripts/remote-deb.sh` builds
from the release commit on a Linux host over SSH (`$NETSCOUT_LINUX_HOST`,
default `reika`). The app
has an ad-hoc signature, not a Developer ID one, so the first launch must be
allowed in *Impostazioni di Sistema → Privacy e sicurezza → Apri comunque*;
the `Leggimi.txt` in the disk image explains it.
