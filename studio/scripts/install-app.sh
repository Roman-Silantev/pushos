#!/bin/sh
# Builds PushOS Studio and puts it in ~/Applications, beside PushOS.
#
# The build directory is removed afterwards, because Studio is rebuilt rarely
# and what a release build leaves behind is many times the size of the app.
# Pass --keep-build to keep it for a quicker rebuild.
set -eu

keep_build=false
for argument in "$@"; do
	case "$argument" in
	--keep-build) keep_build=true ;;
	*)
		echo "usage: $0 [--keep-build]" >&2
		exit 2
		;;
	esac
done

studio=$(cd "$(dirname "$0")/.." && pwd)
identifier=dev.pushos.studio
# Where Cargo builds, unless it has been told to build somewhere else.
target="${CARGO_TARGET_DIR:-$studio/src-tauri/target}"
built="$target/release/bundle/macos/PushOS Studio.app"
applications="$HOME/Applications"
installed="$applications/PushOS Studio.app"
staging="$applications/.PushOS Studio.app.installing"

cd "$studio"
npm run tauri build -- --bundles app

# Only an app that says it is Studio is replaced. A different app that happens
# to have the same name is somebody else's.
if [ -e "$installed" ]; then
	found=$(plutil -extract CFBundleIdentifier raw "$installed/Contents/Info.plist" 2>/dev/null || true)
	if [ "$found" != "$identifier" ]; then
		echo "$installed is not PushOS Studio; leaving it alone" >&2
		exit 1
	fi
fi

# Copied beside the old app and swapped in, so an interrupted install leaves
# the old one working rather than half of the new one.
mkdir -p "$applications"
rm -rf "$staging"
ditto "$built" "$staging"
codesign --verify --strict "$staging"
rm -rf "$installed"
mv "$staging" "$installed"

echo "installed $installed ($(du -sh "$installed" | cut -f1))"

# A directory named by CARGO_TARGET_DIR may be shared with other projects, so
# only Studio's own is removed.
if [ "$keep_build" = false ] && [ -z "${CARGO_TARGET_DIR:-}" ]; then
	echo "removing the build directory ($(du -sh "$target" | cut -f1))"
	rm -rf "$target"
fi
