install:
    cargo build --release --locked --target-dir target
    just _bundle target/release/doohickey target/doohickey.app
    mkdir -p "$HOME/Applications"
    ditto "target/doohickey.app" "$HOME/Applications/doohickey.app"
    codesign --verify --strict "$HOME/Applications/doohickey.app"

# Install doohickey and open it automatically at login.
startup: install
    #!/usr/bin/osascript
    set appPath to (POSIX path of (path to home folder)) & "Applications/doohickey.app"
    tell application "System Events"
        if (count of (login items whose path is appPath)) is 0 then
            make new login item at end of login items with properties {path:appPath, hidden:false}
        end if
    end tell

# Build a drag-to-Applications disk image for Apple silicon.
dmg:
    #!/bin/sh
    set -eu
    cargo build --release --locked --target aarch64-apple-darwin --target-dir target
    work=$(mktemp -d target/.dmg.XXXXXX)
    trap 'rm -rf "$work"' EXIT
    mkdir "$work/image"
    just _bundle target/aarch64-apple-darwin/release/doohickey "$work/image/doohickey.app"
    ln -s /Applications "$work/image/Applications"
    hdiutil create -volname doohickey -srcfolder "$work/image" -fs HFS+ -format UDZO "$work/doohickey.dmg"
    mv -f "$work/doohickey.dmg" target/doohickey-arm64.dmg
    echo "Created target/doohickey-arm64.dmg"

_bundle $binary $app:
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    install -S -m 755 "$binary" "$app/Contents/MacOS/doohickey"
    install -m 644 macos/Info.plist "$app/Contents/"
    install -m 644 macos/Notebook.icns "$app/Contents/Resources/"
    codesign --force --sign - "$app"
    codesign --verify --strict "$app"
