#!/usr/bin/env bash
# Runs the opt-in tests against a real Secret Service (Linux, docs/architecture.md §8).
#
# Each test gets a throwaway D-Bus session whose config has no service directory, so nothing
# can be auto-started. The user's own keyring is never touched: GNOME Keyring runs with its data
# and runtime directories in a temporary folder.
#
#   1. No Secret Service at all: pins down which error that gives.
#   2. A fresh, unlocked GNOME Keyring: a real set / get / delete round trip.
#
# Needs dbus-run-session (package dbus) and gnome-keyring-daemon (package gnome-keyring).
set -euo pipefail

cd "$(dirname "$0")/../src-tauri"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cat >"$tmp/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$tmp</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF

test_cmd=(cargo test -p simplecalendar-core --lib -- --ignored --exact)

# Build first, so the sessions below only run tests.
cargo test -p simplecalendar-core --lib --no-run --quiet

echo "== session without a Secret Service"
dbus-run-session --config-file="$tmp/bus.conf" -- \
	"${test_cmd[@]}" secrets::tests::no_secret_service_is_reported_as_no_service

echo "== session with a fresh, unlocked GNOME Keyring"
# shellcheck disable=SC2016 # expanded by the inner shell
dbus-run-session --config-file="$tmp/bus.conf" -- bash -c '
	set -euo pipefail
	export XDG_DATA_HOME="$1/data" XDG_RUNTIME_DIR="$1/run"
	mkdir -p -m 700 "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR"
	# The password on stdin creates the login keyring and unlocks it.
	printf test | gnome-keyring-daemon --foreground --unlock --components=secrets >/dev/null &
	daemon=$!
	trap "kill $daemon 2>/dev/null || true" EXIT
	for _ in $(seq 50); do
		dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus \
			org.freedesktop.DBus.NameHasOwner string:org.freedesktop.secrets |
			grep -q "boolean true" && break
		sleep 0.1
	done
	shift
	"$@"
' _ "$tmp" "${test_cmd[@]}" secrets::tests::real_keychain_round_trip
