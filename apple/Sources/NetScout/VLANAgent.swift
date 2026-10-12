import Foundation

/// NetScout's root helper for tagged VLANs, one per session.
///
/// Listening to the frames on the cable and creating a VLAN interface both
/// need an administrator. Instead of asking for the password each time,
/// NetScout asks once: the privileged call starts this small shell agent,
/// which stays as root and takes the app's commands until NetScout quits
/// (or crashes), then removes the interfaces it made and its directory.
///
/// Commands go through a directory of the app's (`N.cmd`, one line each),
/// answers come back in a directory of the agent's (`N.res`), owned by
/// root so only the agent can replace them, readable by this user alone.
/// The agent never runs what it reads: it knows four commands, and checks
/// every argument (a tag is a number, a port is `enN`, an address is
/// digits and dots).
///
/// - `up TAG PORT dhcp|manual ADDRESS NETMASK`: makes `vlanN` on PORT and
///   answers "vlanN 172.20.20.250 0xffffff00".
/// - `down vlanN`: removes one it made.
/// - `listen PORT SECONDS`: listens for the tagged frames other devices
///   send on PORT, summed up line by line ("frames TAG COUNT", "mac TAG
///   MAC", "net TAG A.B.C COUNT", then "end") in the file it answers with.
/// - `quiet`: stops listening.
@MainActor
final class VLANAgent {
    private let commands: URL
    private let results: URL
    private var next = 1

    private init(commands: URL, results: URL) {
        self.commands = commands
        self.results = results
    }

    /// Start the agent, asking for the administrator password.
    static func start() async throws -> VLANAgent {
        let files = FileManager.default
        let commands = files.temporaryDirectory.appending(path: "netscout-agent-\(UUID().uuidString)")
        try files.createDirectory(at: commands, withIntermediateDirectories: false,
                                  attributes: [.posixPermissions: 0o700])
        assert(!bootScript.contains("'") && !agentScript.contains("'"), "the scripts are passed inside single quotes")
        let args = [commands.path(percentEncoded: false), String(ProcessInfo.processInfo.processIdentifier), String(getuid())]
        // Every argument is a path in the user's temporary folder or a
        // number; quoted anyway.
        let command = "/bin/sh -c '\(bootScript)' netscout-boot '\(agentScript)' "
            + args.map { "'\($0)'" }.joined(separator: " ")
        let output: String
        do {
            output = try await runPrivileged(command)
        } catch {
            try? files.removeItem(at: commands)
            throw error
        }
        guard output.hasPrefix("/"), files.fileExists(atPath: output) else {
            try? files.removeItem(at: commands)
            throw VLANError.failed("risposta inattesa: \(output)")
        }
        debugLog("vlan agent up: \(output)")
        return VLANAgent(commands: commands, results: URL(filePath: output))
    }

    /// Still running: its directory goes with it.
    var isAlive: Bool { FileManager.default.fileExists(atPath: results.path(percentEncoded: false)) }

    /// Send `words` as one command; the answer's text, or what went wrong.
    func send(_ words: [String], timeout: Duration = .seconds(20)) async throws -> String {
        let n = next
        next += 1
        let pending = commands.appending(path: "\(n).tmp")
        try (words.joined(separator: " ") + "\n").write(to: pending, atomically: false, encoding: .utf8)
        try FileManager.default.moveItem(at: pending, to: commands.appending(path: "\(n).cmd"))
        let answer = results.appending(path: "\(n).res")
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if let text = try? String(contentsOf: answer, encoding: .utf8) {
                let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
                let body = lines.dropFirst().joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
                guard lines.first == "0" else { throw VLANError.failed(body) }
                return body
            }
            guard isAlive else { throw VLANError.failed("l'aiuto con privilegi di NetScout si è chiuso") }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw VLANError.failed("nessuna risposta dall'aiuto con privilegi di NetScout")
    }

    /// Let the agent go: within a moment it removes the interfaces it made
    /// and exits.
    func stop() {
        try? FileManager.default.removeItem(at: commands)
    }

    /// Run as root through the administrator prompt: makes the agent's
    /// directory, starts the agent in the background and prints the
    /// directory. Arguments: the agent script, the commands directory,
    /// NetScout's pid, the user's uid.
    ///
    /// The prompt signals what the command leaves behind once it returns
    /// (nohup alone is not enough): HUP, INT and TERM are ignored, which
    /// carries over to the agent and all it starts.
    private static let bootScript = #"""
        trap "" HUP INT TERM
        out=$(mktemp -d /tmp/netscout-agent.XXXXXX) || exit 1
        chmod 755 "$out"
        /bin/sh -c "$1" netscout-agent "$2" "$3" "$4" "$out" </dev/null >/dev/null 2>&1 &
        echo "$out"
        """#

    /// The agent (see the type's documentation). Arguments: the commands
    /// directory, NetScout's pid, the user's uid, its own directory. No
    /// single quotes in here: it is passed inside them.
    private static let agentScript = #"""
        commands=$1; app=$2; owner=$3; out=$4
        made=""
        listener=""
        summarize="
        {
          tag = \"\"
          for (i = 1; i < NF; i++) if (\$i == \"vlan\") { tag = \$(i + 1); sub(/,\$/, \"\", tag); break }
          if (tag !~ /^[0-9]+\$/) next
          frames[tag]++
          f = frames[tag]
          if (f == 1 || f == 10 || f == 100 || f == 1000 || f == 10000 || f == 100000) print \"frames \" tag \" \" f
          k = tag \" \" \$2
          if (!(k in macs)) { macs[k] = 1; print \"mac \" k }
          ip = \"\"
          if (match(\$0, /ethertype IPv4 \\(0x0800\\), [0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+/)) { n = split(substr(\$0, RSTART, RLENGTH), w, \" \"); ip = w[n] }
          else if (match(\$0, /tell [0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+/)) ip = substr(\$0, RSTART + 5, RLENGTH - 5)
          else if (match(\$0, /Reply [0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+/)) ip = substr(\$0, RSTART + 6, RLENGTH - 6)
          if (ip != \"\" && ip !~ /^(0|127|169\\.254|22[4-9]|23[0-9]|24[0-9]|25[0-5])\\./) {
            split(ip, o, \".\")
            k = tag \" \" o[1] \".\" o[2] \".\" o[3]
            c = ++nets[k]
            if (c == 1 || c == 10 || c == 100 || c == 1000 || c == 10000) print \"net \" k \" \" c
          }
          fflush()
        }
        END { print \"end\"; fflush() }
        "

        number() { case $1 in ""|*[!0-9]*) return 1 ;; esac; [ ${#1} -le 5 ]; }
        port() { case $1 in en[0-9]|en[0-9][0-9]) return 0 ;; esac; return 1; }
        address() { case $1 in ""|*[!0-9.]*) return 1 ;; esac; [ ${#1} -le 15 ]; }

        # A file of the app in the agent directory: the app can read it, nobody
        # else, and only the agent can replace it.
        own() { : > "$1"; chown "$owner" "$1"; chmod 600 "$1"; }

        # The answer to command $1: status ($2, 0 = done) and text ($3).
        reply() {
          own "$out/$1.tmp"
          printf "%s\n%s\n" "$2" "$3" >> "$out/$1.tmp"
          mv "$out/$1.tmp" "$out/$1.res"
        }

        down() { ipconfig set "$1" NONE >/dev/null 2>&1; ifconfig "$1" destroy 2>/dev/null; }

        # The agent ignores HUP, INT and TERM (see VLANAgent.start) and so does
        # all it starts: only KILL stops tcpdump.
        quiet() {
          [ -n "$listener" ] && kill -9 $listener 2>/dev/null
          listener=""
        }

        cleanup() {
          quiet
          for v in $made; do down $v; done
          rm -rf "$out"
        }

        # up N TAG PORT dhcp|manual ADDRESS NETMASK
        up() {
          n=$1; tag=$2; parent=$3; mode=$4; addr=$5; mask=$6
          if ! number "$tag" || [ "$tag" -lt 1 ] || [ "$tag" -gt 4094 ] || ! port "$parent"; then
            reply $n 2 "richiesta non valida"; return
          fi
          if [ "$mode" = manual ] && { ! address "$addr" || ! address "$mask"; }; then
            reply $n 2 "richiesta non valida"; return
          fi
          for v in $(ifconfig -l); do
            case $v in vlan*) ifconfig $v | grep -q "vlan: $tag parent interface: $parent\$" && { reply $n 3 "la VLAN $tag su $parent esiste già ($v)"; return; } ;; esac
          done
          i=0
          while ifconfig vlan$i >/dev/null 2>&1; do i=$((i+1)); done
          v=vlan$i
          ifconfig $v create || { reply $n 1 "impossibile creare $v"; return; }
          if ! ifconfig $v vlan $tag vlandev $parent || ! ifconfig $v up; then
            down $v; reply $n 1 "impossibile collegare $v a $parent"; return
          fi
          if [ "$mode" = dhcp ]; then
            ipconfig set $v DHCP
            ip=""; i=0
            while [ -z "$ip" ] && [ $i -lt 30 ]; do sleep 1; ip=$(ipconfig getifaddr $v); i=$((i+1)); done
            if [ -z "$ip" ]; then
              down $v; reply $n 4 "nessuna risposta dal DHCP della VLAN $tag in 30 secondi"; return
            fi
            m=$(ipconfig getoption $v subnet_mask)
            # Keep the leased address but drop router and DNS: the VLAN must
            # never take over the default route or the name servers.
            ipconfig set $v MANUAL $ip $m
            i=0
            while ! ifconfig $v | grep -q "inet $ip " && [ $i -lt 10 ]; do sleep 1; i=$((i+1)); done
          elif ! ifconfig $v inet $addr netmask $mask; then
            down $v; reply $n 1 "indirizzo non accettato"; return
          fi
          made="$made $v"
          # Not inside "$(...)": there the bash 3.2 behind sh brace-expands the
          # comma in the awk program and splits it in two.
          inet=$(ifconfig $v | awk "/inet /{print \$2, \$4; exit}")
          reply $n 0 "$v $inet"
        }

        # down N INTERFACE: only one this agent made.
        remove() {
          case " $made " in
            *" $2 "*) down $2; made=$(echo " $made " | sed "s/ $2 / /"); reply $1 0 "" ;;
            *) reply $1 2 "interfaccia sconosciuta" ;;
          esac
        }

        # listen N PORT SECONDS: the tagged frames other devices send on PORT,
        # summed up line by line in N.vlans until "end".
        listen() {
          n=$1; parent=$2; secs=$3
          if ! port "$parent" || ! number "$secs" || [ "$secs" -gt 600 ]; then
            reply $n 2 "richiesta non valida"; return
          fi
          quiet
          mac=$(ifconfig $parent | awk "/ether /{print \$2; exit}")
          case $mac in ??:??:??:??:??:??) ;; *) reply $n 1 "$parent non ha un indirizzo MAC"; return ;; esac
          own "$out/$n.vlans"
          rm -f "$out/pipe"; mkfifo "$out/pipe"
          awk "$summarize" < "$out/pipe" >> "$out/$n.vlans" &
          /usr/sbin/tcpdump -l -nn -e -i $parent not ether src $mac and vlan > "$out/pipe" 2>/dev/null &
          listener=$!
          ( sleep $secs; kill -9 $listener 2>/dev/null ) &
          reply $n 0 "$out/$n.vlans"
        }

        n=1
        while kill -0 $app 2>/dev/null && [ -d "$commands" ]; do
          f="$commands/$n.cmd"
          if [ -f "$f" ] && [ ! -L "$f" ]; then
            read -r verb a b c d e < "$f"
            rm -f "$f"
            case $verb in
              up) up $n "$a" "$b" "$c" "$d" "$e" ;;
              down) remove $n "$a" ;;
              listen) listen $n "$a" "$b" ;;
              quiet) quiet; reply $n 0 "" ;;
              *) reply $n 2 "comando sconosciuto" ;;
            esac
            n=$((n+1))
          else
            sleep 0.2
          fi
        done
        cleanup
        """#

    /// Run `command` as root through the system's administrator prompt.
    private static func runPrivileged(_ command: String) async throws -> String {
        let escaped = command
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
        let script = "do shell script \"\(escaped)\" with administrator privileges without altering line endings "
            + "with prompt \"NetScout deve ascoltare la porta Ethernet e creare interfacce VLAN per scansionare le reti taggate.\""
        return try await withCheckedThrowingContinuation { continuation in
            let process = Process()
            process.executableURL = URL(filePath: "/usr/bin/osascript")
            process.arguments = ["-e", script]
            let out = Pipe(), err = Pipe()
            process.standardOutput = out
            process.standardError = err
            process.terminationHandler = { p in
                let stdout = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                let stderr = String(decoding: err.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                if p.terminationStatus == 0 {
                    continuation.resume(returning: stdout.trimmingCharacters(in: .whitespacesAndNewlines))
                } else if stderr.contains("-128") {
                    continuation.resume(throwing: VLANError.cancelled)
                } else {
                    // "osascript: … execution error: <message> (<code>)"
                    var message = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
                    if let r = message.range(of: "execution error: ") { message = String(message[r.upperBound...]) }
                    if let r = message.range(of: #"\s*\(-?\d+\)$"#, options: .regularExpression) { message.removeSubrange(r) }
                    continuation.resume(throwing: VLANError.failed(message))
                }
            }
            do {
                try process.run()
            } catch {
                continuation.resume(throwing: VLANError.failed("\(error)"))
            }
        }
    }
}
