contents := "target/doohickey.app/Contents"

install:
    cargo build --release --locked --target-dir target
    mkdir -p "{{contents}}/MacOS" "{{contents}}/Resources"
    install -S -m 755 target/release/doohickey "{{contents}}/MacOS/"
    install -m 644 macos/Info.plist "{{contents}}/"
    install -m 644 macos/Notebook.icns "{{contents}}/Resources/"
    codesign --force --sign - "target/doohickey.app"
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
