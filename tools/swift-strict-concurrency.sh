#!/usr/bin/env bash
# Swift 6 strict concurrency in the SDK (#630, #1273): what is left, target
# by target. Package.swift is `swift-tools-version:6.0` with
# `swiftLanguageModes: [.v6]`, so `Rutis`, `RutisTest`, `RutisFixturePlugins`
# and `RutisSelfTest` compile in the Swift 6 language mode: every
# strict-concurrency diagnostic is an error and fails the build, and this
# script lists what the log has (and counts the `@unchecked Sendable`
# declarations that say in place why they are safe).
#
# Usage: tools/swift-strict-concurrency.sh
#            build the four targets in a build path of its own (every file
#            compiles, so every diagnostic is printed) and report
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
# macOS only: the targets are Darwin-only, so elsewhere the script is skipped.
set -euo pipefail
cd "$(dirname "$0")/.."

# The targets with the checking on, and the directory each one compiles.
targets=(
  "Rutis:swift/rutis/Sources/Rutis"
  "RutisTest:swift/rutis/Sources/RutisTest"
  "RutisFixturePlugins:swift/rutis/Sources/RutisFixturePlugins"
  "RutisSelfTest:swift/rutis/Tests/SelfTest"
)

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
  echo "skipped: the targets build on macOS only (they need Darwin)"
  exit 0
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/rutis-strict-concurrency.XXXXXX")"
cleanup() {
  rm -rf "$work"
}
trap cleanup EXIT

if [ -z "$log" ]; then
  log="$work/build.log"
  : > "$log"
  # One build path for the four: each target's own files are compiled once,
  # by the build that first reaches them, so every diagnostic is printed
  # once. Targets compile Swift 5 sources, so building one does not rebuild
  # the others that a later build asks for.
  for entry in "${targets[@]}"; do
    target="${entry%%:*}"
    echo "swift build --target $target (a build path of its own: every file compiles)"
    if ! swift build --target "$target" --scratch-path "$work/build" >>"$log" 2>&1; then
      echo "the $target target does not build; the last lines of its log:" >&2
      tail -n 40 "$log" >&2
      exit 1
    fi
  done
elif [ ! -f "$log" ]; then
  echo "no such build log: $log" >&2
  exit 2
fi

# Only the target's own diagnostics: SwiftPM points at files by an absolute
# or a relative path, and the lines of a diagnostic's source excerpt and
# notes start with a space. In the Swift 6 language mode a diagnostic is an
# error, and GitHub Actions makes swiftc write color and hyperlink escapes
# around "error:"; both are stripped before the match.
diagnostics_of() {
  perl -pe 's/\e\[[0-9;]*m//g; s/\e\]8;;[^\e]*\e\\//g' "$log" \
    | grep -E ':[0-9]+:[0-9]+: (warning|error): ' \
    | grep -F "$1/" \
    | grep -Ei 'sendable|actor|isolat|concurren|data race' \
    | LC_ALL=C sort -u || true
}

count_lines() {
  if [ -s "$1" ]; then wc -l < "$1" | tr -d ' '; else echo 0; fi
}

# `@unchecked Sendable` is where the compiler cannot see the isolation: each
# one says in place why it is safe. The count is of declarations; the notes
# around them name the word too.
annotations() {
  find "$1" -name '*.swift' -exec grep -oE '[:,] @unchecked Sendable' {} + 2>/dev/null \
    | wc -l | tr -d ' '
}
annotations_sdk="$(annotations swift/rutis)"

rows=""
counts=""
detail=""
listing=""
count_all=0
annotations_all=0
for entry in "${targets[@]}"; do
  target="${entry%%:*}"
  dir="${entry#*:}"
  list="$work/$target.diagnostics"
  diagnostics_of "$dir" > "$list"
  count="$(count_lines "$list")"
  annotations_target="$(annotations "$dir")"
  count_all=$((count_all + count))
  annotations_all=$((annotations_all + annotations_target))
  rows="$rows| \`$target\` | $count | $annotations_target |"$'\n'
  if [ -z "$counts" ]; then
    counts="$target $count"
  else
    counts="$counts, $target $count"
  fi
  if [ "$count" -gt 0 ]; then
    by_file="$(sed -E "s#^.*${dir}/([^:]+):[0-9]+:[0-9]+: (warning|error):.*#\1#" "$list" \
      | LC_ALL=C sort | uniq -c | sort -rn)"
    if [ -n "$detail" ]; then detail="$detail"$'\n\n'; fi
    detail="$detail\`$target\` ($count):"$'\n\n'"$by_file"
    if [ -n "$listing" ]; then listing="$listing"$'\n\n'; fi
    listing="$listing\`$target\` ($count):"$'\n\n'"$(cat "$list")"
  fi
done

table() {
  echo '| target | diagnostics | `@unchecked Sendable` |'
  echo '| --- | ---: | ---: |'
  printf '%s' "$rows"
  echo "| the four | $count_all | $annotations_all |"
  echo "| \`swift/rutis\` (all of it) | — | $annotations_sdk |"
}

mkdir -p "$(dirname "$report")"
{
  echo "# Swift strict concurrency (#630, #1273)"
  echo
  echo '`Package.swift` is `swift-tools-version:6.0` with '
  echo '`swiftLanguageModes: [.v6]`: the targets `Rutis`, `RutisTest`,'
  echo '`RutisFixturePlugins` and `RutisSelfTest` compile in the Swift 6'
  echo 'language mode, where every strict-concurrency diagnostic is an error'
  echo 'and fails the build.'
  echo
  table
  echo
  if [ "$count_all" -gt 0 ]; then
    echo "Diagnostics by file:"
    echo
    echo '```'
    printf '%s\n' "$detail"
    echo '```'
    echo
    echo "All $count_all:"
    echo
    echo '```'
    printf '%s\n' "$listing"
    echo '```'
  else
    echo "No strict concurrency diagnostics: the four targets meet Swift 6's"
    echo "checking, as far as this compiler reports."
  fi
} > "$report"
cat "$report"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo
    echo "### Swift strict concurrency (#630)"
    echo
    table
    if [ "$count_all" -gt 0 ]; then
      echo
      echo "Diagnostics by file:"
      echo
      echo '```'
      printf '%s\n' "$detail"
      echo '```'
      echo
      echo "The full list is in the step log and in \`$report\`."
    fi
  } >> "$GITHUB_STEP_SUMMARY"
fi

if [ -n "${GITHUB_ACTIONS:-}" ]; then
  echo "::warning title=Swift strict concurrency (#630)::${count_all} diagnostics in the four targets (${counts}); ${annotations_all} @unchecked Sendable left in them, ${annotations_sdk} in swift/rutis"
fi
