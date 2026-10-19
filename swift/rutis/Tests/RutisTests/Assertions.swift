// The tests use Swift Testing, which every Swift toolchain has (XCTest
// comes with Xcode only); these are the assertions they are written with.
import Testing

func XCTAssertEqual<T: Equatable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T, _ message: String = "",
                                  sourceLocation: SourceLocation = #_sourceLocation)
{
    do {
        let (x, y) = (try a(), try b())
        #expect(x == y, "\(message)", sourceLocation: sourceLocation)
    } catch {
        Issue.record(error, "\(message)", sourceLocation: sourceLocation)
    }
}

func XCTAssertTrue(_ value: @autoclosure () throws -> Bool, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) {
    do {
        #expect(try value(), "\(message)", sourceLocation: sourceLocation)
    } catch {
        Issue.record(error, "\(message)", sourceLocation: sourceLocation)
    }
}

func XCTAssertFalse(_ value: @autoclosure () throws -> Bool, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) {
    XCTAssertTrue(!(try value()), message, sourceLocation: sourceLocation)
}

func XCTAssertNil(_ value: @autoclosure () throws -> Any?, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) {
    do {
        let v = try value()
        #expect(v == nil, "\(message) \(String(describing: v))", sourceLocation: sourceLocation)
    } catch {
        Issue.record(error, "\(message)", sourceLocation: sourceLocation)
    }
}

func XCTAssertNotNil(_ value: @autoclosure () throws -> Any?, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) {
    do {
        let v = try value()
        #expect(v != nil, "\(message)", sourceLocation: sourceLocation)
    } catch {
        Issue.record(error, "\(message)", sourceLocation: sourceLocation)
    }
}

func XCTUnwrap<T>(_ value: @autoclosure () throws -> T?, _ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) throws -> T {
    try #require(try value(), "\(message)", sourceLocation: sourceLocation)
}

func XCTFail(_ message: String = "", sourceLocation: SourceLocation = #_sourceLocation) {
    Issue.record("\(message)", sourceLocation: sourceLocation)
}

func XCTAssertThrowsError<T>(_ expression: @autoclosure () throws -> T, _ message: String = "",
                             sourceLocation: SourceLocation = #_sourceLocation, _ handler: (Error) -> Void = { _ in })
{
    do {
        _ = try expression()
        Issue.record("expected an error: \(message)", sourceLocation: sourceLocation)
    } catch {
        handler(error)
    }
}

func XCTAssertNoThrow<T>(_ expression: @autoclosure () throws -> T, _ message: String = "",
                         sourceLocation: SourceLocation = #_sourceLocation)
{
    do {
        _ = try expression()
    } catch {
        Issue.record(error, "\(message)", sourceLocation: sourceLocation)
    }
}
