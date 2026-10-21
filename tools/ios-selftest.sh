#!/usr/bin/env bash
# The Swift SDK's own tests (RutisSelfTest: values, the leaf API, events,
# in-memory sessions, RutisTest's conformance) on the iOS simulator: built
# for it with SwiftPM, run with `simctl spawn` in a simulator made for the
# run and deleted after it. Needs Xcode with an iOS simulator runtime.
#
#   bash tools/ios-selftest.sh
set -euo pipefail
cd "$(dirname "$0")/.."

triple=arm64-apple-ios17.0-simulator
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
build=(swift build --product RutisSelfTest --triple "$triple" --sdk "$sdk" --build-path .build/ios-simulator)
"${build[@]}"
binary="$("${build[@]}" --show-bin-path)/RutisSelfTest"

# The newest iOS runtime, and an iPhone it supports.
read -r runtime kind < <(xcrun simctl list runtimes -j | python3 -c '
import json, sys
runtimes = [r for r in json.load(sys.stdin)["runtimes"] if r["platform"] == "iOS" and r["isAvailable"]]
if not runtimes:
    sys.exit("no iOS simulator runtime: install one with `xcodebuild -downloadPlatform iOS`")
runtime = max(runtimes, key=lambda r: [int(n) for n in r["version"].split(".")])
iphone = [t for t in runtime["supportedDeviceTypes"] if t["productFamily"] == "iPhone"][-1]
print(runtime["identifier"], iphone["identifier"])
')

udid=$(xcrun simctl create "rutis-selftest-$$" "$kind" "$runtime")
trap 'xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true; xcrun simctl delete "$udid" || true' EXIT
xcrun simctl boot "$udid"
xcrun simctl bootstatus "$udid" -b >/dev/null
xcrun simctl spawn "$udid" "$binary"
