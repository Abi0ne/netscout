import AppKit
import Foundation

/// Opens an SSH or Telnet session to a host in Terminal.app, logging in with
/// the given user and password.
///
/// The session is an `expect` script that Terminal runs through a settings
/// file (`.terminal`, no Automation permission needed). `expect` answers the
/// login and password prompts, hides everything before the remote prompt and
/// then hands the session to the user; the window closes when it ends. The password never appears on a command line or on screen:
/// it is only in the script, which is private to the user (0700, in the
/// per-user temporary directory) and deletes itself as soon as it starts.
enum TerminalLauncher {
    /// The protocols Terminal can open.
    static func supports(_ kind: RemoteProtocol) -> Bool { kind != .rdp }

    /// The Telnet client; macOS no longer ships one (Homebrew: `brew install telnet`).
    static var telnetPath: String? {
        ["/opt/homebrew/bin/telnet", "/usr/local/bin/telnet", "/usr/bin/telnet"]
            .first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    enum LaunchError: LocalizedError {
        case telnetMissing
        case terminalMissing
        case unsupported(RemoteProtocol)

        var errorDescription: String? {
            switch self {
            case .telnetMissing:
                "Telnet non è installato su questo Mac. Installalo con Homebrew: brew install telnet"
            case .terminalMissing:
                "Impossibile trovare l'app Terminale."
            case .unsupported(let kind):
                "Il Terminale non apre sessioni \(kind.label)."
            }
        }
    }

    static func open(_ kind: RemoteProtocol, host: String, port: UInt16, user: String, password: String) throws {
        let command: String
        switch kind {
        case .ssh:
            // accept-new: first contact with a LAN device needs no yes/no
            // question; a *changed* key is still refused.
            let login = user.isEmpty ? "" : " -l $user"
            command = "spawn ssh -o StrictHostKeyChecking=accept-new -p \(port)\(login) \(host)"
        case .telnet:
            guard let telnet = telnetPath else { throw LaunchError.telnetMissing }
            command = "spawn \(telnet) \(host) \(port)"
        case .rdp:
            throw LaunchError.unsupported(kind)
        }
        guard let terminal = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") else {
            throw LaunchError.terminalMissing
        }

        let dir = FileManager.default.temporaryDirectory.appending(path: "NetScout", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        let script = dir.appending(path: "\(kind.rawValue)-\(host)-\(UUID().uuidString.prefix(8)).exp")
        let title = "\(kind.label) \(user.isEmpty ? "" : "\(user)@")\(host)"
        let text = expectScript(kind: kind, title: title, spawn: command, user: user, password: password)
        guard FileManager.default.createFile(atPath: script.path(percentEncoded: false),
                                             contents: Data(text.utf8),
                                             attributes: [.posixPermissions: 0o700]) else {
            throw CocoaError(.fileWriteUnknown)
        }
        // The script deletes itself when it runs; this covers Terminal never running it.
        DispatchQueue.main.asyncAfter(deadline: .now() + 60) {
            try? FileManager.default.removeItem(at: script)
        }

        let settings = dir.appending(path: "NetScout.terminal")
        try windowSettings(running: script.path(percentEncoded: false)).write(to: settings)
        NSWorkspace.shared.open([settings], withApplicationAt: terminal, configuration: NSWorkspace.OpenConfiguration())
    }

    /// A Terminal settings file that runs `command` directly (no shell, so no
    /// command line echoed and no "Last login") and closes the window when it
    /// exits. It starts from the user's default profile so the window looks
    /// like their other ones. Terminal keeps it as a profile named after the
    /// file, "NetScout", replaced on every launch.
    private static func windowSettings(running command: String) throws -> Data {
        let prefs = UserDefaults(suiteName: "com.apple.Terminal")
        let profiles = prefs?.dictionary(forKey: "Window Settings")
        var settings = prefs?.string(forKey: "Default Window Settings")
            .flatMap { profiles?[$0] as? [String: Any] } ?? [:]
        settings["name"] = "NetScout"
        settings["type"] = "Window Settings"
        settings["CommandString"] = command
        settings["RunCommandAsShell"] = true
        settings["shellExitAction"] = 0 // close the window, however the session ended
        return try PropertyListSerialization.data(fromPropertyList: settings, format: .xml, options: 0)
    }

    /// The session script. Everything before the remote prompt stays hidden;
    /// then the session is handed to the user. An error that ends the session
    /// before that is shown until the user presses Return (the window closes
    /// with the session). `title`, the user and the password are passed as
    /// character codes, which keeps any quote, brace or `$` in them inert; the
    /// host is an IPv4 address from the scan and the port a number.
    private static func expectScript(kind: RemoteProtocol, title: String, spawn: String, user: String, password: String) -> String {
        let loginPrompt = kind == .telnet ? #"""
                -nocase -re {(login|username|user name)[^:\n]*: ?$} {
                    if {$user eq ""} { hand_over $expect_out(buffer) }
                    send -- "$user\r"
                    exp_continue
                }
            """# : ""
        return #"""
            #!/usr/bin/expect -f
            # NetScout: \#(kind.label) session. This file deletes itself.
            file delete -- [info script]
            proc decode {codes} { set s ""; foreach c $codes { append s [format %c $c] }; return $s }
            set user [decode {\#(tclCodes(user))}]
            set pw [decode {\#(tclCodes(password))}]
            set title [decode {\#(tclCodes(title))}]

            # Clear the window (and its scrollback) and name it.
            log_user 0
            send_user "\033\[H\033\[2J\033\[3J\033\]0;$title\007"

            # Show `text` and give the session to the user.
            proc hand_over {text} {
                send_user -- [string trimleft $text "\r\n"]
                interact
                exit
            }
            proc last_line {text} {
                return [lindex [split [string trimleft $text "\r\n"] "\n"] end]
            }
            # The session ended before the prompt: show why, wait for Return.
            proc fail {text} {
                send_user -- [string trim $text "\r\n"]
                send_user "\n\n\[Premi Invio per chiudere\]"
                set timeout -1
                expect_user -re "\n" {} eof {}
                exit 1
            }

            set timeout 20
            set sent 0
            \#(spawn)
            expect {
                -re {\(yes/no[^)]*\)\? ?$} {
                    # Host-key question: the user answers.
                    send_user -- [string trimleft $expect_out(buffer) "\r\n"]
                    set timeout -1
                    expect_user -re "(.*)\n" { send -- "$expect_out(1,string)\r" } eof { exit }
                    set timeout 20
                    exp_continue
                }
            \#(loginPrompt)
                -nocase -re {password[^:\n]*: ?$} {
                    if {$pw ne "" && !$sent} {
                        send -- "$pw\r"
                    } else {
                        # No password given, or the one given was refused:
                        # the user types it, unseen.
                        if {$sent} { send_user "Password errata.\n" }
                        send_user -- [last_line $expect_out(buffer)]
                        stty -echo
                        set timeout -1
                        expect_user -re "(.*)\n" { send -- "$expect_out(1,string)\r" } eof { exit }
                        stty echo
                        send_user "\n"
                        set timeout 20
                    }
                    set sent 1
                    exp_continue
                }
                -re {(?:^|\n)([^\n]*[#$>%] ?)$} {
                    # The prompt: show just that line.
                    hand_over $expect_out(1,string)
                }
                timeout {
                    # No recognisable prompt: show what arrived and hand over.
                    expect -timeout 0 -re {.+} { hand_over $expect_out(0,string) }
                    hand_over ""
                }
                eof { fail $expect_out(buffer) }
            }

            """#
    }

    private static func tclCodes(_ s: String) -> String {
        s.unicodeScalars.map { String($0.value) }.joined(separator: " ")
    }
}
