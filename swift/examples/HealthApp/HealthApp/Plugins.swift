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

// `provides` is optional (what `ctx.provide` provides reaches rutis by
// itself); declared here for now: a service of a dialed-in connection that
// is not declared does not yet reach a row of rutis's own in the same
// instance (a loader fix is under way), and goes once it lands.
let health = Rutis.define(Plugin(
    name: "health",
    provides: ["health": .of(Health.self, sync: ["status"])],
    apply: { ctx in ctx.provide("health", Health()) }))

func device(installation: String) -> Rutis.Definition {
    Rutis.define(Plugin(
        name: "device",
        provides: ["device": .of(Device.self, sync: ["id"])],
        apply: { ctx in ctx.provide("device", Device(installation: installation)) }))
}
