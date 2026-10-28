// A sample app hosting rutis plugins (docs/guide/swift-app.md): its business
// layer connects while the app is in the foreground and stops when it goes
// to the background; once connected the app installs its plugins, each with
// its configuration, and rutis uses their services.
//
// Where to connect comes from the app's settings, which launch arguments
// set too (`-RutisConnect wss://… -RutisID kim-iphone -RutisToken …`, in
// Xcode's scheme or `xcrun simctl launch`); `-RutisCA` is a CA certificate
// to trust (DER, base64) for a rutis with a certificate of its own CA.
import Rutis
import Security
import SwiftUI

enum Settings {
    static var defaults: UserDefaults { .standard }

    static var connect: String { defaults.string(forKey: "RutisConnect") ?? "ws://127.0.0.1:7443/rutis" }

    /// This installation's id: given, or made once and kept.
    static var id: String {
        if let id = defaults.string(forKey: "RutisID"), !id.isEmpty { return id }
        let made = "app-" + UUID().uuidString.prefix(8).lowercased()
        defaults.set(made, forKey: "RutisID")
        return made
    }

    /// A real app keeps the token in the Keychain.
    static var token: String { defaults.string(forKey: "RutisToken") ?? "" }

    static var trust: [SecCertificate] {
        guard let text = defaults.string(forKey: "RutisCA"), let der = Data(base64Encoded: text),
              let certificate = SecCertificateCreateWithData(nil, der as CFData)
        else { return [] }
        return [certificate]
    }
}

/// Lines for whoever reads the app's output (Xcode's console, the tests).
func say(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
}

@main
struct HealthApp: App {
    @Environment(\.scenePhase) private var phase
    @State private var rutis: RutisApp = {
        let id = Settings.id
        // Installed once connected, in this order: `health` with its
        // defaults, `device` with the configuration given here.
        return RutisApp(plugins: [health, device], configs: ["device": DeviceConfig(installation: id)],
                        connect: Settings.connect, id: id, token: Settings.token, trust: Settings.trust)
    }()

    var body: some Scene {
        WindowGroup {
            ContentView(rutis: rutis)
                .task {
                    for await state in rutis.states { say("app: state \(state)") }
                }
        }
        .onChange(of: phase) { _, phase in
            switch phase {
            case .active:
                Task {
                    do {
                        try await rutis.start()
                    } catch {
                        say("app: cannot start: \(error)")
                    }
                }
            case .background:
                // Time for the plugins' cleanups before iOS suspends the app.
                let task = UIApplication.shared.beginBackgroundTask()
                Task {
                    await rutis.stop()
                    UIApplication.shared.endBackgroundTask(task)
                }
            default:
                break
            }
        }
    }
}

struct ContentView: View {
    let rutis: RutisApp

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "heart.text.square").font(.system(size: 48))
            Text("Rutis Health").font(.title)
            Text("rutis: \(rutis.state.description)").font(.headline)
            Text(Settings.connect).font(.footnote).foregroundStyle(.secondary)
        }
        .padding()
    }
}
