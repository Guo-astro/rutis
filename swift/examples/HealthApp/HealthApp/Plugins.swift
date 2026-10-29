// The plugins this app offers rutis: ordinary rutis plugins, written as any
// Swift plugin is (docs/guide/swift-plugin.md), compiled into the app.
import HealthKit
import Rutis
import UIKit

/// Thrown when steps cannot be read: the caller gets it, in any language,
/// with its name, message and `code`.
struct HealthAccessDenied: Error, LocalizedError {
    /// `unavailable` (no Health on this device), `not-determined` (not
    /// asked yet) or `denied`.
    let code: String
    var errorDescription: String? { "step counts are not shared with this app (\(code))" }
}

/// The `health` service: today's steps, from HealthKit.
@RutisService
final class Health: @unchecked Sendable {
    let store = HKHealthStore()
    let steps = HKQuantityType(.stepCount)

    /// Where access stands: `unavailable`, `not-determined`, `denied` or
    /// `authorized`.
    func status() -> String {
        guard HKHealthStore.isHealthDataAvailable() else { return "unavailable" }
        switch store.authorizationStatus(for: steps) {
        case .notDetermined: return "not-determined"
        case .sharingDenied: return "denied"
        case .sharingAuthorized: return "authorized"
        @unknown default: return "unknown"
        }
    }

    /// Asks the user (the prompt names this app); whether access was
    /// granted.
    func authorize() async throws -> Bool {
        guard HKHealthStore.isHealthDataAvailable() else { throw HealthAccessDenied(code: "unavailable") }
        try await store.requestAuthorization(toShare: [steps], read: [steps])
        return store.authorizationStatus(for: steps) == .sharingAuthorized
    }

    /// Today's steps.
    func stepsToday() async throws -> Double {
        let status = status()
        guard status == "authorized" else { throw HealthAccessDenied(code: status) }
        let today = HKQuery.predicateForSamples(withStart: Calendar.current.startOfDay(for: Date()), end: Date())
        let query = HKStatisticsQueryDescriptor(predicate: HKSamplePredicate.quantitySample(type: steps, predicate: today),
                                                options: .cumulativeSum)
        return try await query.result(for: store)?.sumQuantity()?.doubleValue(for: .count()) ?? 0
    }
}

/// The `device` service: which installation and which device this is.
/// rutis does not model devices; rows that need to know ask this service.
@RutisService
final class Device: @unchecked Sendable {
    let installation: String
    init(installation: String) { self.installation = installation }

    /// The id this app connects to rutis with.
    func id() -> String { installation }

    func model() async -> String { await UIDevice.current.model }

    func system() async -> String {
        await "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)"
    }
}

// What `ctx.provide` provides reaches rutis by itself: no `provides` needed.
/// The `health` plugin's configuration: the app gives it in `RutisApp`.
@RutisConfig
struct HealthConfig: Codable {
    /// The name it provides its service as: `health-<installation>`, so
    /// that two devices on one rutis do not clash.
    var service = "health"
}

let health = Rutis.define(Plugin(name: "health", config: HealthConfig.self) { ctx, config in
    ctx.provide(config.service, Health())
})

/// The `device` plugin's configuration: the app gives it in `RutisApp`.
@RutisConfig
struct DeviceConfig: Codable {
    /// The id this app connects to rutis with.
    var installation: String
    /// The name it provides its service as: `device-<installation>`.
    var service = "device"
}

let device = Rutis.define(Plugin(name: "device", config: DeviceConfig.self) { ctx, config in
    ctx.provide(config.service, Device(installation: config.installation))
})

/// The plugins' configurations for the installation `id`: its services are
/// `health-<id>` and `device-<id>`.
func configs(_ id: String) -> [String: any Encodable] {
    ["health": HealthConfig(service: "health-\(id)"),
     "device": DeviceConfig(installation: id, service: "device-\(id)")]
}
