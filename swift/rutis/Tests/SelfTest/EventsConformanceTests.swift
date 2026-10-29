// guarantee: events/one-bus
// The event expectations (#464 §13): the real runtimes and the testing
// tools are checked against the same list. What each expectation means is
// crates/rutis-bridge/src/runtime/testing/events.rs; this runner follows
// go/rutis/rutistest/events_conformance_test.go.
import Foundation
@testable import Rutis
import RutisFixturePlugins
@testable import RutisTest

/// The language of the Swift testing tool in the file.
private let eventLanguage = "swift"
/// How long a log expectation may take to hold (events.rs SETTLE).
private let eventSettle = 5.0
/// How long `logLacks` / `reported` wait after the rest held.
private let eventLacks = 0.3

/// events-log, the host's service the participants log through.
@RutisService
final class EventsLogService: @unchecked Sendable {
    private let lock = NSLock()
    private var entries: [String] = []

    func add(_ entry: String) { lock.withLock { entries.append(entry) } }

    /// Logs filter:<row> of the description.
    func filter(_ description: Rutis.Advanced.Listener) { add("filter:" + (description.row ?? "")) }

    var list: [String] { lock.withLock { entries } }
}

private struct Outcome {
    var value: Value?
    var error: Error?
}

private final class EventRun: @unchecked Sendable {
    let caseJSON: JSON
    let caseID: String
    let name: String
    let log: EventsLogService
    var root: Loaded!
    var rows: [String: Loaded] = [:]
    var participants: [String: JSON] = [:]
    var generation = 0
    var pending: Task<Outcome, Never>?
    var reported = 0

    init(caseJSON: JSON, log: EventsLogService) {
        self.caseJSON = caseJSON
        caseID = caseJSON["case"]?.stringValue ?? ""
        name = caseJSON["name"]?.stringValue ?? ""
        self.log = log
    }

    func native(_ json: JSON) -> Any? {
        switch json {
        case .null: return nil
        case let .array(items): return items.map(native)
        case let .object(fields):
            var out: [String: Any?] = [:]
            for (key, value) in fields { out[key] = .some(native(value)) }
            return out
        default: return Value.data(json)
        }
    }

    /// Loads a listener row.
    func load(_ listener: JSON) async throws {
        guard case var .object(fields) = listener else { return }
        if listener["name"] == nil { fields.append(("name", .string(name))) }
        let label = listener["label"]?.stringValue ?? ""
        let config = JSON.object(fields)
        rows[label] = try await root.load(config: Value.data(config), row: label)
        participants[label] = config
    }

    /// Changes the config of the row `label`, so it registers again.
    func update(_ label: String) async throws {
        guard case var .object(fields) = participants[label] ?? .null else { throw RutisError(name: "Error", message: "no listener row \(label)") }
        generation += 1
        fields.removeAll { $0.0 == "generation" }
        fields.append(("generation", .int(generation)))
        let config = JSON.object(fields)
        try await rows[label]!.update(config: Value.data(config))
        participants[label] = config
    }

    /// The emitter row of `label`, created on first use.
    func emitter(_ label: String) async throws -> (Loaded, String) {
        if rows[label] == nil {
            rows[label] = try await root.load(config: Value.data(.object([("label", .string(label)), ("emitter", .bool(true))])),
                                              row: label)
        }
        return (rows[label]!, label)
    }

    /// A value as the expectations write it, as the session carries it.
    func fromWritten(_ json: JSON) -> Value {
        if case let .object(fields) = json, fields.count == 1, fields[0].0.hasPrefix("$") {
            let tag = String(fields[0].0.dropFirst())
            let inner = fields[0].1
            switch tag {
            case "callback": return Rutis._Generated.export { (text: String) in text.uppercased() }.value
            case "map": return .map((inner.arrayValue ?? []).map { pair in
                    (fromWritten(pair.arrayValue![0]), fromWritten(pair.arrayValue![1]))
                })
            case "set": return .set((inner.arrayValue ?? []).map(fromWritten))
            default: return (try? parseTagged(tag, inner.stringValue ?? "")) ?? .undefined
            }
        }
        switch json {
        case let .array(items): return .list(of: items.map(fromWritten))
        case let .object(fields): return .record(of: fields.map { ($0.0, fromWritten($0.1)) })
        default: return .data(json)
        }
    }

    func emit(_ spec: JSON) async -> Outcome {
        let row = spec["row"]?.stringValue ?? "e"
        let event = spec["name"]?.stringValue ?? name
        do {
            let (loaded, key) = try await emitter(row)
            let args = (spec["args"]?.arrayValue ?? []).map(fromWritten)
            var request: [(String, Value)] = [("mode", .data(spec["mode"] ?? .null)), ("name", .data(.string(event))), ("args", .list(of: args))]
            for field in ["filter", "terminal", "cancel"] {
                if let value = spec[field] { request.append((field, .data(value))) }
            }
            let mode = spec["mode"]?.stringValue ?? ""
            let method = mode == "bail" ? "dispatchSync" : "dispatch"
            let service = try await loaded.service("emitter-" + key)
            let value = try await service.callValue(method, [.record(of: request)])
            return Outcome(value: value)
        } catch {
            return Outcome(error: error)
        }
    }

    // ── Expectations ─────────────────────────────────────────────────

    func matches(_ error: Error, _ want: JSON) -> String? {
        guard let remote = error as? RutisError else { return "not an error of the plugin: \(error)" }
        if let name = want["name"]?.stringValue, remote.name != name { return "name \(remote.name), want \(name) (\(remote))" }
        if let message = want["message"]?.stringValue, !remote.message.contains(message) {
            return "message \"\(remote.message)\", want \"\(message)\""
        }
        if let children = want["errors"]?.arrayValue {
            guard remote.errors.count == children.count else { return "errors of \(remote): \(remote.errors.count), want \(children.count)" }
            for (i, child) in children.enumerated() {
                if let problem = matches(remote.errors[i], child) { return "errors[\(i)]: \(problem)" }
            }
        }
        return nil
    }

    func checkOutcomes(_ where_: String, _ expect: JSON, _ outcomes: [Outcome]) {
        if expect["cancelled"] == .bool(true) {
            record("\(where_): cancelled: the testing tool does not give up a dispatch")
            return
        }
        if expect["returns"] != nil || expect["throws"] != nil, outcomes.isEmpty {
            record("\(where_): expects a result, but no dispatch ends in this step")
            return
        }
        for outcome in outcomes {
            if expect["throws"] == nil, let error = outcome.error {
                record("\(where_): expected the dispatch to succeed, it threw \(error)")
                continue
            }
            if let returns = expect["returns"], outcome.error == nil {
                let got = written(outcome.value ?? .undefined)
                if got != returns { record("\(where_): got \(JSONWriter.string(got)), want \(JSONWriter.string(returns))") }
            }
        }
        if let thrown = expect["throws"] {
            var problem: String? = "expected an error, got \(outcomes.map { $0.value.map { JSONWriter.string(written($0)) } ?? "\($0.error!)" })"
            for outcome in outcomes {
                guard let error = outcome.error else { continue }
                problem = matches(error, thrown)
                if problem == nil { break }
            }
            if let problem { record("\(where_): \(problem)") }
        }
    }

    /// The error name of a §5.6 line about `event` naming `row`, or nil:
    /// as the runtime writes it, or as the host does (`rutis-host/test`'s
    /// `reported`).
    static func reports(_ line: String, _ event: String, _ filter: Bool, _ row: String) -> String? {
        let prefixes = filter
            ? ["rutis: filter of \(event) from row \(row) failed: ", "filter of \(event) from row \(row) failed: "]
            : ["rutis: listener of \(event) in row \(row) failed: ", "\(row): listener of \(event) failed: "]
        guard let prefix = prefixes.first(where: { line.hasPrefix($0) }) else { return nil }
        let rest = line.dropFirst(prefix.count)
        guard let colon = rest.range(of: ": ") else { return nil }
        return String(rest[..<colon.lowerBound])
    }

    func logProblems(_ expect: JSON, _ event: String) async -> String? {
        let log = self.log.list
        var out: [String] = []
        func strings(_ key: String) -> [String]? { expect[key]?.arrayValue?.compactMap(\.stringValue) }
        if let want = strings("log"), log != want { out.append("log \(log), want \(want)") }
        if let want = strings("logUnordered"), log.sorted() != want.sorted() { out.append("log \(log), want in any order \(want)") }
        if let want = strings("logContains") {
            var i = 0
            for entry in log where i < want.count && entry == want[i] { i += 1 }
            if i != want.count { out.append("log \(log), want it to contain \(want)") }
        }
        for lacking in strings("logLacks") ?? [] where log.contains(lacking) { out.append("log \(log) has \(lacking)") }
        if let want = strings("filterSees") {
            let seen = log.filter { $0.hasPrefix("filter:") }.map { String($0.dropFirst(7)) }
            if seen != want { out.append("the filter saw \(seen), want \(want)") }
        }
        let lines = Array(await root.reported.dropFirst(reported))
        for want in expect["reported"]?.arrayValue ?? [] {
            let label = want["label"]?.stringValue ?? ""
            let filter = want["filter"] == .bool(true)
            let about = lines.filter { EventRun.reports($0, event, filter, label) != nil }
            if about.count != 1 {
                out.append("reported \(lines): want one line for \(label) about \(event), got \(about.count)")
            } else if EventRun.reports(about[0], event, filter, label) != want["name"]?.stringValue {
                out.append("reported \(about[0]): \(label) should fail with \(want["name"]?.stringValue ?? "")")
            }
        }
        return out.isEmpty ? nil : out.joined(separator: "; ")
    }

    func checkLog(_ where_: String, _ expect: JSON, _ event: String) async {
        let deadline = Date().addingTimeInterval(eventSettle)
        while let problem = await logProblems(expect, event) {
            if Date() > deadline {
                record("\(where_): \(problem)")
                return
            }
            try? await Task.sleep(nanoseconds: 10_000_000)
        }
        if expect["logLacks"] != nil || expect["reported"] != nil {
            try? await Task.sleep(nanoseconds: UInt64(eventLacks * 1e9))
            if let problem = await logProblems(expect, event) { record("\(where_): \(problem) (after the step had settled)") }
        }
    }

    func waitFor(_ label: String) async throws {
        let deadline = Date().addingTimeInterval(eventSettle)
        while !log.list.contains(label) {
            if Date() > deadline { throw RutisError(name: "Error", message: "\(label) did not start: \(log.list)") }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
    }

    func step(_ index: Int, _ step: JSON) async throws {
        let where_ = "\(caseID) step \(index)"
        let expect = step["expect"] ?? .object([])
        let event = step["emit"]?["name"]?.stringValue ?? name
        var outcomes: [Outcome] = []
        let started = Date()
        if let spec = step["emit"], step["background"] == .bool(true) {
            pending = Task { await self.emit(spec) }
            await checkLog(where_, expect, event)
            return
        } else if let spec = step["emit"] {
            outcomes.append(await emit(spec))
        } else if let listener = step["load"] {
            try await load(listener)
        } else if let label = step["update"]?.stringValue {
            try await update(label)
        } else if let label = step["unload"]?.stringValue {
            if let when = step["when"]?.stringValue { try await waitFor(when) }
            try await rows[label]!.unload()
            if let pending {
                self.pending = nil
                let waiter = Task { await pending.value }
                let timer = Task {
                    try await Task.sleep(nanoseconds: UInt64(eventSettle * 1e9))
                    waiter.cancel()
                }
                let outcome = await waiter.value
                timer.cancel()
                outcomes.append(outcome)
            }
            if expect["settles"] == .bool(true), Date().timeIntervalSince(started) >= eventSettle {
                record("\(where_): did not settle within \(eventSettle)s")
            }
        } else {
            record("\(where_): a step the testing tool cannot take: \(JSONWriter.string(step))")
            return
        }
        checkOutcomes(where_, expect, outcomes)
        await checkLog(where_, expect, event)
    }
}

struct EventsConformanceTests {
    /// The labels of a case (events.rs `places`): its listeners, its loaded
    /// participants and its emitter rows.
    func labels(_ caseJSON: JSON) -> Set<String> {
        var labels = Set<String>()
        for listener in caseJSON["listeners"]?.arrayValue ?? [] { labels.insert(listener["label"]?.stringValue ?? "") }
        func emitRows(_ step: JSON) -> [String] {
            if let emit = step["emit"] { return [emit["row"]?.stringValue ?? "e"] }
            return (step["concurrent"]?.arrayValue ?? []).flatMap(emitRows)
        }
        for step in caseJSON["steps"]?.arrayValue ?? [] {
            if let load = step["load"] { labels.insert(load["label"]?.stringValue ?? "") }
            for row in emitRows(step) { labels.insert(row) }
        }
        return labels
    }

    /// Whether a limit (an only / except item) holds with every label at X
    /// in Swift (events.rs `limit_holds`).
    func holds(_ limit: JSON, _ labels: Set<String>) -> Bool {
        for (key, value) in limit.objectValue ?? [] where key != "why" {
            let languages = value.arrayValue?.compactMap(\.stringValue) ?? []
            if !(key == "X" || key == "Y" || labels.contains(key)) || !languages.contains(eventLanguage) { return false }
        }
        return true
    }

    /// Whether a case places a row in a scope: the testing tool loads rows
    /// at the root only.
    func scoped(_ caseJSON: JSON) -> Bool {
        func inScope(_ json: JSON?) -> Bool { (json?["scope"]?.stringValue ?? "root") != "root" }
        func emits(_ step: JSON) -> [JSON] {
            if let emit = step["emit"] { return [emit] }
            return (step["concurrent"]?.arrayValue ?? []).flatMap(emits)
        }
        if (caseJSON["listeners"]?.arrayValue ?? []).contains(where: inScope) { return true }
        for step in caseJSON["steps"]?.arrayValue ?? [] {
            if inScope(step["load"]) || emits(step).contains(where: inScope) { return true }
        }
        return false
    }

    /// Whether a case runs on the Swift testing tool (events.rs `tool_combo`).
    func forSwift(_ caseJSON: JSON) -> Bool {
        if caseJSON["tool"] == .bool(false) || scoped(caseJSON) { return false }
        let labels = labels(caseJSON)
        for limit in caseJSON["only"]?.arrayValue ?? [] where !holds(limit, labels) { return false }
        for limit in caseJSON["except"]?.arrayValue ?? [] where holds(limit, labels) { return false }
        return true
    }

    func rutisTestMeetsTheEventExpectations() async throws {
        let expected = try expectations("events.expected.json")
        let only = ProcessInfo.processInfo.environment["RUTIS_EVENTS_CASE"]
        var ran = 0
        for caseJSON in expected["cases"]!.arrayValue! where forSwift(caseJSON) {
            ran += 1
            let id = caseJSON["case"]!.stringValue!
            if let only, !only.split(separator: ",").contains(where: { id.hasPrefix($0) }) { continue }
            resetConformanceLog()
            let log = EventsLogService()
            let run = EventRun(caseJSON: caseJSON, log: log)
            run.root = try await RutisTest.load(conformanceEventsPlugin, config: Value.data(.object([("label", .string("e")), ("emitter", .bool(true))])),
                                                services: ["events-log": log], row: "e")
            run.rows["e"] = run.root
            run.reported = await run.root.reported.count
            do {
                for listener in caseJSON["listeners"]?.arrayValue ?? [] { try await run.load(listener) }
                for (i, step) in (caseJSON["steps"]?.arrayValue ?? []).enumerated() { try await run.step(i, step) }
            } catch {
                record("\(id): \(error)")
            }
            await run.root.close()
        }
        // The skip logic agrees with events.rs `tool_combo`.
        expect(ran == expected["toolCases"]?[eventLanguage]?.int64Value.map(Int.init), "ran \(ran) cases")
    }
}
