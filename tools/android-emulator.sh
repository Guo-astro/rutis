#!/usr/bin/env bash
# An Android emulator for the `android` CI job (docs/ci.md): the Android
# SDK, a system image, an AVD of the job's own, booted headless.
#
#   start  the SDK: ANDROID_HOME when it has the emulator and the image,
#          else one installed with Google's command-line tools in
#          ~/.cache/rutis-ci/android-sdk (kept between jobs); then an AVD in
#          the job's temporary directory, booted on a free console port
#          (several runner instances share a machine), waited for until the
#          system booted. Writes ANDROID_HOME, ANDROID_SERIAL and PATH to
#          GITHUB_ENV / GITHUB_PATH when they are set.
#   stop   ends the emulator this job started.
#
# An arm64 image on Apple silicon (x86_64 elsewhere); it needs a hypervisor
# (Hypervisor.framework, KVM): the self-hosted Macs have one, GitHub's
# hosted macOS runners do not. RUTIS_ANDROID_API (default 34) and
# RUTIS_ANDROID_TAG (default google_apis) choose the image.
#
# Usage: tools/android-emulator.sh start|stop
set -euo pipefail

phase=${1:?usage: $0 start|stop}
temp=${RUNNER_TEMP:-${TMPDIR:-/tmp}}
pidfile="$temp/rutis-android-emulator.pid"

if [ "$phase" = stop ]; then
  if [ -f "$pidfile" ]; then
    kill "$(cat "$pidfile")" 2>/dev/null || true
    rm -f "$pidfile"
  fi
  exit 0
fi
[ "$phase" = start ] || { echo "usage: $0 start|stop" >&2; exit 2; }

api=${RUTIS_ANDROID_API:-34}
tag=${RUTIS_ANDROID_TAG:-google_apis}
case "$(uname -m)" in
  arm64 | aarch64) abi=arm64-v8a ;;
  *) abi=x86_64 ;;
esac
image="system-images;android-$api;$tag;$abi"
image_dir="system-images/android-$api/$tag/$abi"

# An SDK that has what is needed, or the one this script keeps.
sdk=${ANDROID_HOME:-}
if [ -z "$sdk" ] || [ ! -x "$sdk/emulator/emulator" ] || [ ! -d "$sdk/$image_dir" ] || [ ! -d "$sdk/platforms/android-34" ]; then
  sdk="$HOME/.cache/rutis-ci/android-sdk"
  tools="$sdk/cmdline-tools/latest/bin/sdkmanager"
  if [ ! -x "$tools" ]; then
    case "$(uname -s)" in
      Darwin) zip=commandlinetools-mac-11076708_latest.zip ;;
      *) zip=commandlinetools-linux-11076708_latest.zip ;;
    esac
    mkdir -p "$sdk/cmdline-tools"
    curl -fsSL --retry 3 -o "$temp/$zip" "https://dl.google.com/android/repository/$zip"
    rm -rf "$sdk/cmdline-tools/latest" "$temp/cmdline-tools"
    unzip -q "$temp/$zip" -d "$temp"
    mv "$temp/cmdline-tools" "$sdk/cmdline-tools/latest"
  fi
  yes | "$tools" --sdk_root="$sdk" --licenses >/dev/null || true
  "$tools" --sdk_root="$sdk" --install platform-tools emulator "platforms;android-34" "build-tools;34.0.0" "$image" >/dev/null
fi
export ANDROID_HOME="$sdk"
adb="$sdk/platform-tools/adb"
"$adb" start-server >/dev/null

# An AVD of this job's own.
export ANDROID_AVD_HOME="$temp/avd"
mkdir -p "$ANDROID_AVD_HOME/rutis-ci.avd"
cat > "$ANDROID_AVD_HOME/rutis-ci.ini" <<EOF
avd.ini.encoding=UTF-8
path=$ANDROID_AVD_HOME/rutis-ci.avd
target=android-$api
EOF
cat > "$ANDROID_AVD_HOME/rutis-ci.avd/config.ini" <<EOF
avd.ini.encoding=UTF-8
abi.type=$abi
hw.cpu.arch=${abi%%-*}
hw.cpu.ncore=2
hw.ramSize=2048
disk.dataPartition.size=4G
hw.lcd.width=1080
hw.lcd.height=2400
hw.lcd.density=420
hw.keyboard=yes
image.sysdir.1=$image_dir/
tag.id=$tag
EOF

# A free console port and the adb port after it.
port=
for candidate in $(seq 5554 2 5680); do
  if ! (echo >"/dev/tcp/127.0.0.1/$candidate") 2>/dev/null && ! (echo >"/dev/tcp/127.0.0.1/$((candidate + 1))") 2>/dev/null; then
    port=$candidate
    break
  fi
done
[ -n "$port" ] || { echo "no free emulator port" >&2; exit 1; }
serial="emulator-$port"

"$sdk/emulator/emulator" -avd rutis-ci -port "$port" -no-window -no-audio -no-boot-anim \
  -no-snapshot -skip-adb-auth -gpu swiftshader_indirect >"$temp/rutis-android-emulator.log" 2>&1 &
echo $! >"$pidfile"

# Booted: five minutes at most.
for _ in $(seq 1 150); do
  if [ "$("$adb" -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; then
    for scale in window_animation_scale transition_animation_scale animator_duration_scale; do
      "$adb" -s "$serial" shell settings put global "$scale" 0
    done
    echo "$serial booted (android-$api, $tag, $abi)"
    if [ -n "${GITHUB_ENV:-}" ]; then
      {
        echo "ANDROID_HOME=$sdk"
        echo "ANDROID_SDK_ROOT=$sdk"
        echo "ANDROID_SERIAL=$serial"
      } >>"$GITHUB_ENV"
      echo "$sdk/platform-tools" >>"$GITHUB_PATH"
    fi
    exit 0
  fi
  if ! kill -0 "$(cat "$pidfile")" 2>/dev/null; then
    echo "the emulator ended:" >&2
    tail -50 "$temp/rutis-android-emulator.log" >&2
    exit 1
  fi
  sleep 2
done
echo "$serial did not boot in five minutes:" >&2
tail -50 "$temp/rutis-android-emulator.log" >&2
exit 1
