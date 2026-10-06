//! Remote sessions: SSH and Telnet in a terminal emulator, RDP in Remmina.
//!
//! With `expect` installed the session runs through the same script as the
//! macOS app: it answers the login and password prompts, hides everything
//! before the remote prompt and then hands the session to the user. The
//! password never appears on a command line or on screen: it is only in the
//! script, which is private to the user (0700, in `$XDG_RUNTIME_DIR`) and
//! deletes itself as soon as it starts. Without `expect` the client runs
//! directly and the user types the password in the terminal.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use netscout_core::Port;

/// A remote-session protocol a scanned port can be opened with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    Ssh,
    Telnet,
    Rdp,
}

impl Remote {
    /// The protocol a port speaks, if it is one we can open a session to.
    pub fn for_port(port: &Port) -> Option<Self> {
        match (
            port.service.as_deref().map(str::to_lowercase).as_deref(),
            port.number,
        ) {
            (Some("ssh"), _) | (_, 22) => Some(Self::Ssh),
            (Some("telnet"), _) | (_, 23) => Some(Self::Telnet),
            (Some("ms-wbt-server" | "rdp"), _) | (_, 3389) => Some(Self::Rdp),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ssh => "SSH",
            Self::Telnet => "Telnet",
            Self::Rdp => "Desktop remoto (RDP)",
        }
    }

    pub fn button_label(self) -> &'static str {
        match self {
            Self::Ssh | Self::Telnet => "Terminale",
            Self::Rdp => "Remmina",
        }
    }

    pub fn help(self) -> String {
        match self {
            Self::Ssh | Self::Telnet => format!("Apri una sessione {} nel terminale", self.label()),
            Self::Rdp => "Apri il desktop remoto in Remmina".into(),
        }
    }
}

/// Why a session of `kind` cannot be opened here, if it cannot.
pub fn unavailable_reason(kind: Remote) -> Option<String> {
    match kind {
        Remote::Rdp => find_in_path("remmina")
            .is_none()
            .then(|| "Remmina non è installato. Installalo con: sudo apt install remmina".into()),
        Remote::Telnet if find_in_path("telnet").is_none() => {
            Some("Telnet non è installato. Installalo con: sudo apt install telnet".into())
        }
        Remote::Ssh if find_in_path("ssh").is_none() => Some(
            "Il client SSH non è installato. Installalo con: sudo apt install openssh-client"
                .into(),
        ),
        _ => terminal()
            .is_none()
            .then(|| "Nessun emulatore di terminale trovato.".into()),
    }
}

/// True if the password can be passed to the session (`expect` installed).
pub fn can_send_password() -> bool {
    find_in_path("expect").is_some()
}

/// Open the RDP session in Remmina.
pub fn open_rdp(host: &str, port: u16) -> Result<(), String> {
    let remmina = find_in_path("remmina").ok_or("Remmina non è installato.")?;
    let address = if port == 3389 {
        format!("rdp://{host}")
    } else {
        format!("rdp://{host}:{port}")
    };
    spawn(Command::new(remmina).arg("-c").arg(address))
}

/// Open an SSH or Telnet session in a terminal window.
pub fn open(kind: Remote, host: &str, port: u16, user: &str, password: &str) -> Result<(), String> {
    let (term, prefix) = terminal().ok_or("Nessun emulatore di terminale trovato.")?;
    let client = match kind {
        Remote::Ssh => {
            // accept-new: first contact with a LAN device needs no yes/no
            // question; a *changed* key is still refused.
            let mut args = vec![
                "ssh".to_string(),
                "-o".into(),
                "StrictHostKeyChecking=accept-new".into(),
                "-p".into(),
                port.to_string(),
            ];
            if !user.is_empty() {
                args.extend(["-l".into(), user.to_string()]);
            }
            args.push(host.to_string());
            args
        }
        Remote::Telnet => {
            let telnet = find_in_path("telnet").ok_or("Telnet non è installato.")?;
            vec![
                telnet.to_string_lossy().into_owned(),
                host.to_string(),
                port.to_string(),
            ]
        }
        Remote::Rdp => return open_rdp(host, port),
    };

    let command: Vec<String> = match find_in_path("expect") {
        Some(expect) => {
            let title = format!(
                "{} {}{host}",
                kind.label(),
                if user.is_empty() {
                    String::new()
                } else {
                    format!("{user}@")
                }
            );
            let script = write_script(&expect_script(kind, &title, &client, user, password))?;
            vec![
                expect.to_string_lossy().into_owned(),
                "-f".into(),
                script.to_string_lossy().into_owned(),
            ]
        }
        // No expect: run the client; if it fails, keep the window open so
        // the error can be read.
        None => [
            "sh",
            "-c",
            r#""$@" || { printf '\n[Premi Invio per chiudere]'; read _; }"#,
            "sh",
        ]
        .into_iter()
        .map(String::from)
        .chain(client)
        .collect(),
    };
    let mut cmd = Command::new(term);
    cmd.args(prefix).args(command);
    spawn(&mut cmd)
}

/// Start `cmd` detached; a thread reaps it so it leaves no zombie.
fn spawn(cmd: &mut Command) -> Result<(), String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Impossibile avviare {:?}: {e}", cmd.get_program()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The terminal emulator to use and the arguments that precede the command.
/// The desktop's own terminal comes first.
fn terminal() -> Option<(PathBuf, &'static [&'static str])> {
    const GNOME: &[(&str, &[&str])] = &[
        ("ptyxis", &["--new-window", "--"]),
        ("kgx", &["--"]),
        ("gnome-terminal", &["--"]),
    ];
    const KDE: &[(&str, &[&str])] = &[("konsole", &["-e"])];
    const OTHERS: &[(&str, &[&str])] = &[
        ("xfce4-terminal", &["-x"]),
        ("mate-terminal", &["-x"]),
        ("lxterminal", &["-e"]),
        ("alacritty", &["-e"]),
        ("kitty", &[]),
        ("foot", &[]),
        ("wezterm", &["start", "--"]),
        ("xterm", &["-e"]),
        ("x-terminal-emulator", &["-e"]),
    ];
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    let lists: [&[(&str, &[&str])]; 3] = if desktop.contains("kde") {
        [KDE, GNOME, OTHERS]
    } else {
        [GNOME, KDE, OTHERS]
    };
    lists
        .iter()
        .flat_map(|l| l.iter())
        .find_map(|(name, prefix)| find_in_path(name).map(|p| (p, *prefix)))
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Write the script to a private file (0700 directory and file).
fn write_script(text: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    let dir = gtk::glib::user_runtime_dir().join("NetScout");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("Impossibile creare {}: {e}", dir.display()))?;
    let name = format!(
        "session-{}-{}.exp",
        std::process::id(),
        gtk::glib::real_time()
    );
    let path = dir.join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)
        .map_err(|e| format!("Impossibile scrivere {}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("Impossibile scrivere {}: {e}", path.display()))?;
    // The script deletes itself when it runs; this covers it never running.
    let p = path.clone();
    gtk::glib::timeout_add_seconds_local_once(60, move || {
        let _ = std::fs::remove_file(p);
    });
    Ok(path)
}

/// The session script. Everything before the remote prompt stays hidden;
/// then the session is handed to the user. An error that ends the session
/// before that is shown until the user presses Return. `title`, the user and
/// the password are passed as character codes, which keeps any quote, brace
/// or `$` in them inert; the host is an IPv4 address from the scan and the
/// port a number.
fn expect_script(
    kind: Remote,
    title: &str,
    client: &[String],
    user: &str,
    password: &str,
) -> String {
    let login_prompt = if kind == Remote::Telnet {
        r#"    -nocase -re {(login|username|user name)[^:\n]*: ?$} {
        if {$user eq ""} { hand_over $expect_out(buffer) }
        send -- "$user\r"
        exp_continue
    }
"#
    } else {
        ""
    };
    SCRIPT
        .replace("@LABEL@", kind.label())
        .replace("@USER@", &tcl_codes(user))
        .replace("@PW@", &tcl_codes(password))
        .replace("@TITLE@", &tcl_codes(title))
        .replace("@SPAWN@", &format!("spawn {}", client.join(" ")))
        .replace("@LOGIN_PROMPT@\n", login_prompt)
}

fn tcl_codes(s: &str) -> String {
    s.chars()
        .map(|c| (c as u32).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

const SCRIPT: &str = r#"#!/usr/bin/expect -f
# NetScout: @LABEL@ session. This file deletes itself.
file delete -- [info script]
proc decode {codes} { set s ""; foreach c $codes { append s [format %c $c] }; return $s }
set user [decode {@USER@}]
set pw [decode {@PW@}]
set title [decode {@TITLE@}]

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
@SPAWN@
expect {
    -re {\(yes/no[^)]*\)\? ?$} {
        # Host-key question: the user answers.
        send_user -- [string trimleft $expect_out(buffer) "\r\n"]
        set timeout -1
        expect_user -re "(.*)\n" { send -- "$expect_out(1,string)\r" } eof { exit }
        set timeout 20
        exp_continue
    }
@LOGIN_PROMPT@
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
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_encodes_secrets_and_spawns_client() {
        let s = expect_script(
            Remote::Ssh,
            "SSH root@10.0.0.1",
            &["ssh".into(), "-p".into(), "22".into(), "10.0.0.1".into()],
            "root",
            "p$w{",
        );
        assert!(s.contains("spawn ssh -p 22 10.0.0.1"));
        assert!(s.contains("set pw [decode {112 36 119 123}]"));
        assert!(!s.contains("p$w{"));
        assert!(!s.contains("@LOGIN_PROMPT@"));
        assert!(!s.contains("(login|username"));
        let t = expect_script(Remote::Telnet, "t", &["telnet".into()], "", "");
        assert!(t.contains("(login|username"));
    }
}
