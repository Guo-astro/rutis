// The plugin binary answers for its own access to what macOS protects
// (calendars, contacts, automation: TCC), whoever started it (design §8):
// the prompt names the binary and its Info.plist's usage descriptions, and
// what the user grants applies to the binary alone, not to the host or the
// terminal that started it.
//
// macOS attributes a process's requests to its "responsible" process,
// which a child inherits from its parent. A process cannot give that up
// itself, but it can run its own image again in place (posix_spawn with
// POSIX_SPAWN_SETEXEC: the same pid, the same descriptors, channel
// included), disclaiming the parent's responsibility as it does. The two
// functions are libsystem's (since macOS 10.14), not in its headers: looked
// up at run time, and when one is missing nothing changes.
#if os(macOS)
import Darwin
import Foundation

enum Responsibility {
    private typealias ResponsibleFor = @convention(c) (pid_t) -> pid_t
    private typealias SetDisclaim = @convention(c) (UnsafeMutablePointer<posix_spawnattr_t?>, Int32) -> Int32

    /// Set in the image run again, so that it never runs itself a second
    /// time.
    static let variable = "RUTIS_RESPONSIBLE"

    private static func symbol<T>(_ name: String, as _: T.Type) -> T? {
        let all = UnsafeMutableRawPointer(bitPattern: -2) // RTLD_DEFAULT
        return dlsym(all, name).map { unsafeBitCast($0, to: T.self) }
    }

    /// The process responsible for this one, when the system says.
    static func responsible() -> pid_t? {
        symbol("responsibility_get_pid_responsible_for_pid", as: ResponsibleFor.self).map { $0(getpid()) }
    }

    /// Unless this process is responsible for itself, run this binary again
    /// in its place, responsible for itself; returns when it cannot.
    static func claim() {
        if ProcessInfo.processInfo.environment[variable] != nil {
            unsetenv(variable)
            return
        }
        guard let responsible = responsible(), responsible != getpid(),
              let disclaim = symbol("responsibility_spawnattrs_setdisclaim", as: SetDisclaim.self)
        else { return }
        var attributes: posix_spawnattr_t?
        guard posix_spawnattr_init(&attributes) == 0 else { return }
        defer { posix_spawnattr_destroy(&attributes) }
        guard posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETEXEC)) == 0,
              disclaim(&attributes, 1) == 0
        else { return }
        var path = [CChar](repeating: 0, count: Int(MAXPATHLEN))
        var size = UInt32(path.count)
        guard _NSGetExecutablePath(&path, &size) == 0 else { return }
        setenv(variable, "1", 1)
        let arguments = CommandLine.unsafeArgv
        // Returns only when it failed: this image goes on as it was.
        _ = posix_spawn(nil, path, nil, &attributes, arguments, environ)
        unsetenv(variable)
    }
}
#else
enum Responsibility {
    static func claim() {}
}
#endif
