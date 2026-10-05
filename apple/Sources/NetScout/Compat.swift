import NetScoutCore
import SwiftUI

// Building with the Command Line Tools only (no Xcode):
//
// * In the macOS 27 SDK `@State` resolves to a SwiftUI macro whose compiler
//   plugin (SwiftUIMacros) ships only with Xcode. The `State` property wrapper
//   behind it is still public, so views reach it through this alias.
typealias ViewState = SwiftUI.State

// * Foundation also defines `Host`, `Scanner`, `Progress` and `Port` (NSHost,
//   NSScanner, NSProgress, NSPort); in this module the names mean the engine's types.
typealias Host = NetScoutCore.Host
typealias Scanner = NetScoutCore.Scanner
typealias Progress = NetScoutCore.Progress
typealias Port = NetScoutCore.Port
