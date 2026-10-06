# linux/ — Linux app

GTK4 + libadwaita app in Rust. It links `netscout-core` directly as a crate
(no UniFFI), so it uses exactly the engine the macOS app uses. It is a Cargo
project of its own, excluded from the workspace, so `cargo build` at the
root keeps working on hosts without GTK.

Requires GTK 4.16+ and libadwaita 1.7+ (Ubuntu 25.04+, Fedora 42+, Debian 13+).

```
sudo apt install libgtk-4-dev libadwaita-1-dev
cargo run --manifest-path linux/Cargo.toml -- --scan   # scan at launch
NETSCOUT_DEBUG=1 cargo run --manifest-path linux/Cargo.toml  # diagnostics on stderr
scripts/build-deb.sh                                   # dist/netscout_<version>_<arch>.deb
```

## Layout

```
linux/
├── Cargo.toml
├── data/                 # .desktop, AppStream metainfo, hicolor icons
└── src/
    ├── main.rs           # AdwApplication, CSS, app.quit
    ├── window.rs         # window, scan state, engine bridge, sidebar
    ├── window/
    │   ├── profiles.rs   # saved-profiles page, save/rename/delete
    │   ├── compare.rs    # comparison dialog
    │   └── connect.rs    # SSH/Telnet login dialog
    ├── table.rs          # sortable, filterable GtkColumnView of hosts
    ├── detail.rs         # device card, Wake-on-LAN button
    ├── terminal.rs       # terminal emulator / Remmina launch, expect script
    ├── help.rs           # Guida (F1) and About
    ├── labels.rs         # Italian labels, icons, formatting
    ├── config.rs         # ~/.config/NetScout/settings.ini
    └── style.css
```

## Notes

- **Threading** — engine callbacks arrive on the core's runtime threads; a
  `ScanObserver` forwards them over an `async-channel` to a future on the GTK
  main loop. Host updates are coalesced every 150 ms, like the macOS app.
- **Table updates** — a changed host replaces its row object (GTK does not
  re-bind an item it already shows) and the selection is restored.
- **Data** — profiles in `~/.local/share/NetScout/Profiles` (same JSON as
  macOS: they can be copied between the two), settings in
  `~/.config/NetScout/settings.ini`.
- **Remote sessions** — SSH/Telnet open in the desktop's terminal (Ptyxis,
  Console, GNOME Terminal, Konsole, xterm, …). With `expect` installed the
  password is passed by the same self-deleting script as on macOS; without
  it the user types it in the terminal. RDP opens in Remmina.
- **Not ported** — the GitHub self-updater and the Merlin integration are
  macOS-only; on Linux updates come with the package.
