contents := "target/doohickey.app/Contents"

app:
    cargo build --release --locked --target-dir target
    mkdir -p "{{contents}}/MacOS" "{{contents}}/Resources"
    install -S -m 755 target/release/doohickey "{{contents}}/MacOS/"
    install -m 644 macos/Info.plist "{{contents}}/"
    install -m 644 macos/Notebook.icns "{{contents}}/Resources/"
    codesign --force --sign - "target/doohickey.app"
