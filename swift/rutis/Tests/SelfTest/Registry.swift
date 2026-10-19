// The self-test runner's tests, in order (convert_tests.py lists them;
// a new test is added here).

let registry: [(String, () async throws -> Void)] = [
    ("PeerTests.testCallsAndCallbacks", { try await PeerTests().testCallsAndCallbacks() }),
    ("PeerTests.testCancellingAnAsyncCallAbortsTheCallee", { try await PeerTests().testCancellingAnAsyncCallAbortsTheCallee() }),
    ("PeerTests.testAValueTooDeepFailsOnlyItsCall", { try await PeerTests().testAValueTooDeepFailsOnlyItsCall() }),
    ("PeerTests.testTheSessionEndsWhenTheFarEndGoes", { try await PeerTests().testTheSessionEndsWhenTheFarEndGoes() }),
    ("JSONTests.testReadsAndWritesData", { try JSONTests().testReadsAndWritesData() }),
    ("JSONTests.testRefusesWhatIsNotJSON", { JSONTests().testRefusesWhatIsNotJSON() }),
    ("JSONTests.testSurrogatePairs", { try JSONTests().testSurrogatePairs() }),
    ("JSONTests.testDoublesKeepTheirBits", { try JSONTests().testDoublesKeepTheirBits() }),
    ("TaggedValueTests.testDatesAndTimes", { try TaggedValueTests().testDatesAndTimes() }),
    ("TaggedValueTests.testInstants", { try TaggedValueTests().testInstants() }),
    ("TaggedValueTests.testBigIntegers", { TaggedValueTests().testBigIntegers() }),
    ("TaggedValueTests.testBytesDecimalsAndUUIDs", { try TaggedValueTests().testBytesDecimalsAndUUIDs() }),
    ("TaggedValueTests.testMapKeys", { try TaggedValueTests().testMapKeys() }),
    ("CodecTests.testStructFieldsCrossAsTheirValues", { try CodecTests().testStructFieldsCrossAsTheirValues() }),
    ("CodecTests.testPlainDataIsData", { try CodecTests().testPlainDataIsData() }),
    ("CodecTests.testIntegersBeyondTheSafeRangeAreBigInts", { try CodecTests().testIntegersBeyondTheSafeRangeAreBigInts() }),
    ("CodecTests.testNonFiniteNumbersDoNotCross", { CodecTests().testNonFiniteNumbersDoNotCross() }),
    ("CodecTests.testAnyDecodesIntoNativeValues", { try CodecTests().testAnyDecodesIntoNativeValues() }),
    ("CodecTests.testDecimalKeepsItsTextAsAString", { try CodecTests().testDecimalKeepsItsTextAsAString() }),
    ("CodecTests.testCyclicValuesFail", { CodecTests().testCyclicValuesFail() }),
    ("CodecTests.testOptionalsAndMissingFields", { try CodecTests().testOptionalsAndMissingFields() }),
    ("ErrorGraphTests.testAnErrorCrossesWithItsCodeAndData", { ErrorGraphTests().testAnErrorCrossesWithItsCodeAndData() }),
    ("ErrorGraphTests.testAnAggregateKeepsItsChildrenInOrder", { ErrorGraphTests().testAnAggregateKeepsItsChildrenInOrder() }),
]
