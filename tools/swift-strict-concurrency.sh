#!/usr/bin/env bash
# Swift 6 strict concurrency in the SDK (#630): what is left in the `Rutis`
# target. Package.swift turns the checking on for that target
# (`.enableUpcomingFeature("StrictConcurrency")`); the Swift 5 language mode
# makes every diagnostic a warning, so this is a report and not a gate. The
# target moves to Swift 6 when the count reaches zero; `RutisTest`, the
# fixtures and RutisSelfTest follow, target by target.
#
# Usage: tools/swift-strict-concurrency.sh
#            build `Rutis` in a build path of its own (every file compiles,
#            so every diagnostic is printed) and report
#        tools/swift-strict-concurrency.sh --log FILE
#            report from an existing `swift build` log instead (the CI
#            step reads the log of the build the job already ran)
#        tools/swift-strict-concurrency.sh --report FILE
#            where the report goes (default:
#            .build/strict-concurrency-report.txt)
#
# Prints the counts and the diagnostics by file, writes the report with the
# full list, and on GitHub Actions adds it to the step summary and as a
# warning annotation. A report exits 0; only a build that fails exits 1.
# macOS only: the target is Darwin-only, so elsewhere the script is skipped.
set -euo pipefail
cd "$(dirname "$0")/.."

log=""
report=".build/strict-concurrency-report.txt"
while [ $# -gt 0 ]; do
  case "$1" in
    --log) log="${2:?--log needs a file}"; shift 2 ;;
    --report) report="${2:?--report needs a file}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ "$(uname -s)" != Darwin ]; then
  echo "skipped: the Rutis target builds on macOS only (it needs Darwin)"
  exit 0
fi

target_dir="swift/rutis/Sources/Rutis"
scratch=""
cleanup() {
  if [ -n "$scratch" ]; then rm -rf "$scratch"; fi
}
trap cleanup EXIT

if [ -z "$log" ]; then
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/rutis-strict-concurrency.XXXXXX")"
  log="$scratch/build.log"
  echo "swift build --target Rutis (a build path of its own: every file compiles)"
  if ! swift build --target Rutis --scratch-path "$scratch" >"$log" 2>&1; then
    echo "the Rutis target does not build; the last lines of its log:" >&2
    tail -n 40 "$log" >&2
    exit 1
  fi
elif [ ! -f "$log" ]; then
  echo "no such build log: $log" >&2
  exit 2
fi

# Only the target's own diagnostics: SwiftPM points at files by an absolute
# or a relative path, and the lines of a diagnostic's source excerpt and
# notes start with a space.
diagnostics="$(grep -E ':[0-9]+:[0-9]+: warning: ' "$log" \
  | grep -v '^[[:space:]]' \
  | grep -F "$target_dir/" \
  | grep -Ei 'sendable|actor|isolat|concurren|data race' \
  | LC_ALL=C sort -u || true)"
count_diagnostics=0
if [ -n "$diagnostics" ]; then
  count_diagnostics="$(printf '%s\n' "$diagnostics" | wc -l | tr -d ' ')"
fi

# `@unchecked Sendable` is how the code says "trust me" today: each one is
# still to be removed or replaced by a real isolation.
annotations() {
  find "$1" -name '*.swift' -exec grep -o '@unchecked Sendable' {} + 2>/dev/null \
    | wc -l | tr -d ' '
}
annotations_target="$(annotations "$target_dir")"
annotations_sdk="$(annotations swift/rutis)"

by_file=""
if [ -n "$diagnostics" ]; then
  by_file="$(printf '%s\n' "$diagnostics" \
    | sed -E "s#^.*${target_dir}/([^:]+):[0-9]+:[0-9]+: warning:.*#\1#" \
    | LC_ALL=C sort | uniq -c | sort -rn)"
fi

mkdir -p "$(dirname "$report")"
{
  echo "# Swift strict concurrency (#630)"
  echo
  echo '`Package.swift` turns it on for the target `Rutis`'
  echo '(`.enableUpcomingFeature("StrictConcurrency")`) in the Swift 5'
  echo 'language mode: every diagnostic is a warning and the build passes.'
  echo
  echo "| what | count |"
  echo "| --- | ---: |"
  echo "| diagnostics in \`Rutis\` | $count_diagnostics |"
  echo "| \`@unchecked Sendable\` left in \`Rutis\` | $annotations_target |"
  echo "| \`@unchecked Sendable\` left in \`swift/rutis\` | $annotations_sdk |"
  echo
  if [ -n "$by_file" ]; then
    echo "Diagnostics by file:"
    echo
    echo '```'
    printf '%s\n' "$by_file"
    echo '```'
    echo
    echo "All $count_diagnostics:"
    echo
    echo '```'
    printf '%s\n' "$diagnostics"
    echo '```'
  else
    echo "No strict concurrency diagnostics: the target meets Swift 6's"
    echo "checking, as far as this compiler reports."
  fi
} > "$report"
cat "$report"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo
    echo "### Swift strict concurrency (#630)"
    echo
    echo "| what | count |"
    echo "| --- | ---: |"
    echo "| diagnostics in \`Rutis\` | $count_diagnostics |"
    echo "| \`@unchecked Sendable\` left in \`Rutis\` | $annotations_target |"
    echo "| \`@unchecked Sendable\` left in \`swift/rutis\` | $annotations_sdk |"
    if [ -n "$by_file" ]; then
      echo
      echo "Diagnostics by file:"
      echo
      echo '```'
      printf '%s\n' "$by_file"
      echo '```'
      echo
      echo "The full list is in the step log and in \`$report\`."
    fi
  } >> "$GITHUB_STEP_SUMMARY"
fi

if [ -n "${GITHUB_ACTIONS:-}" ]; then
  echo "::warning title=Swift strict concurrency (#630)::${count_diagnostics} diagnostics in Rutis; ${annotations_target} @unchecked Sendable left in it, ${annotations_sdk} in swift/rutis"
fi
