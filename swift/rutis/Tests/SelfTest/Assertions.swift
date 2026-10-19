// The assertions the tests are written with, on the harness (Harness.swift).

func XCTAssertEqual<T: Equatable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T, _ message: String = "",
                                  file: StaticString = #fileID, line: UInt = #line)
{
    do {
        let (x, y) = (try a(), try b())
        expect(x == y, "\(x) != \(y) \(message)", file: file, line: line)
    } catch {
        record(error, message, file: file, line: line)
    }
}

func XCTAssertTrue(_ value: @autoclosure () throws -> Bool, _ message: String = "", file: StaticString = #fileID, line: UInt = #line) {
    expect(try value(), message, file: file, line: line)
}

func XCTAssertFalse(_ value: @autoclosure () throws -> Bool, _ message: String = "", file: StaticString = #fileID, line: UInt = #line) {
    expect(!(try value()), message, file: file, line: line)
}

func XCTAssertNil(_ value: @autoclosure () throws -> Any?, _ message: String = "", file: StaticString = #fileID, line: UInt = #line) {
    do {
        let v = try value()
        expect(v == nil, "\(message) \(String(describing: v))", file: file, line: line)
    } catch {
        record(error, message, file: file, line: line)
    }
}

func XCTAssertNotNil(_ value: @autoclosure () throws -> Any?, _ message: String = "", file: StaticString = #fileID, line: UInt = #line) {
    expect(try value() != nil, message, file: file, line: line)
}

func XCTUnwrap<T>(_ value: @autoclosure () throws -> T?, _ message: String = "", file: StaticString = #fileID, line: UInt = #line) throws -> T {
    try require(try value(), message, file: file, line: line)
}

func XCTFail(_ message: String = "", file: StaticString = #fileID, line: UInt = #line) {
    record(message, file: file, line: line)
}

func XCTAssertThrowsError<T>(_ expression: @autoclosure () throws -> T, _ message: String = "",
                             file: StaticString = #fileID, line: UInt = #line, _ handler: (Error) -> Void = { _ in })
{
    do {
        _ = try expression()
        record("expected an error: \(message)", file: file, line: line)
    } catch {
        handler(error)
    }
}

func XCTAssertNoThrow<T>(_ expression: @autoclosure () throws -> T, _ message: String = "",
                         file: StaticString = #fileID, line: UInt = #line)
{
    do {
        _ = try expression()
    } catch {
        record(error, message, file: file, line: line)
    }
}
