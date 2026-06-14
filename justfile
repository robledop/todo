name := "cosmic-applet-outlook-tasks"
appid := "dev.robledop.OutlookTasks"
prefix := "/usr"

bin-src := "target/release/" + name
bin-dst := prefix + "/bin/" + name
desktop-dst := prefix + "/share/applications/" + appid + ".desktop"
icon-dst := prefix + "/share/icons/hicolor/scalable/apps/" + appid + "-symbolic.svg"
metainfo-dst := prefix + "/share/metainfo/" + appid + ".metainfo.xml"

# Build the release binary. Pass the client id via the env var.
build-release:
    cargo build --release -p {{name}}

run:
    cosmic applet run -p {{name}}

check:
    cargo clippy --workspace --all-targets -- -W clippy::pedantic

test:
    cargo test --workspace

# Install the binary and data files (run with sudo for a system prefix).
install:
    install -Dm0755 {{bin-src}} {{bin-dst}}
    install -Dm0644 applet/data/{{appid}}.desktop {{desktop-dst}}
    install -Dm0644 applet/data/icons/{{appid}}-symbolic.svg {{icon-dst}}
    install -Dm0644 applet/data/{{appid}}.metainfo.xml {{metainfo-dst}}

uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{icon-dst}} {{metainfo-dst}}
