# Camera Center task runner. `just --list` shows all recipes.

set shell := ["bash", "-euo", "pipefail", "-c"]
set windows-shell := ["bash", "-euo", "pipefail", "-c"]

host := os()
target_dir := justfile_directory() / "target"
dist := justfile_directory() / "dist"

# macOS signing team for the camera extension (Xcode → Settings → Accounts).
team_id := env("TEAM_ID", "")

default:
    @just --list

# Build every platform the current host can build (others are skipped; CI builds all three).
build: build-osx build-windows build-linux

# Build the macOS app bundle (Apple Silicon, macOS 26+), including the camera extension.
build-osx:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{host}}" != "macos" ]; then
        echo "skipping build-osx: requires a macOS host (Xcode)"; exit 0
    fi
    cargo build --release --locked --target aarch64-apple-darwin -p cc-app
    signing=(CODE_SIGNING_ALLOWED=NO)
    if [ -n "{{team_id}}" ]; then
        signing=(DEVELOPMENT_TEAM="{{team_id}}" -allowProvisioningUpdates)
    else
        echo "note: TEAM_ID not set, building an unsigned bundle (the camera extension won't activate)"
    fi
    xcodebuild -quiet \
        -project platform/macos/CameraCenter.xcodeproj \
        -target CameraCenter -configuration Release \
        SYMROOT="{{target_dir}}/xcode" \
        RUST_BINARY="{{target_dir}}/aarch64-apple-darwin/release/camera-center" \
        MODELS_DIR="{{justfile_directory()}}/models" \
        "${signing[@]}" build
    mkdir -p "{{dist}}/macos"
    rm -rf "{{dist}}/macos/CameraCenter.app"
    ditto "{{target_dir}}/xcode/Release/CameraCenter.app" "{{dist}}/macos/CameraCenter.app"
    echo "built {{dist}}/macos/CameraCenter.app"


# Build the Windows x64 binaries (app + Media Foundation source DLL).
build-windows:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{host}}" != "windows" ]; then
        echo "skipping build-windows: requires a Windows host (MSVC toolchain)"; exit 0
    fi
    cargo build --release --locked --target x86_64-pc-windows-msvc -p cc-app -p cc-mf-source
    out="{{dist}}/windows"
    mkdir -p "$out/models"
    cp "{{target_dir}}/x86_64-pc-windows-msvc/release/camera-center.exe" "$out/"
    cp "{{target_dir}}/x86_64-pc-windows-msvc/release/cc_mf_source.dll" "$out/"
    cp models/models.toml models/*.onnx "$out/models/"
    echo "built $out (register the camera once: regsvr32 cc_mf_source.dll, as administrator)"

# Build the Linux x64 binary.
build-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{host}}" != "linux" ]; then
        echo "skipping build-linux: requires a Linux host"; exit 0
    fi
    cargo build --release --locked --target x86_64-unknown-linux-gnu -p cc-app
    out="{{dist}}/linux"
    mkdir -p "$out/models"
    cp "{{target_dir}}/x86_64-unknown-linux-gnu/release/camera-center" "$out/"
    cp models/models.toml models/*.onnx "$out/models/"
    echo "built $out"

# Install the app for the current host platform.
install:
    just install-{{ if host == "macos" { "osx" } else { host } }}

# Uninstall the app for the current host platform.
uninstall:
    just uninstall-{{ if host == "macos" { "osx" } else { host } }}

# Install to /Applications (required for camera extension activation). Set TEAM_ID to sign.
install-osx: build-osx
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "macos" ] || { echo "install-osx requires a macOS host"; exit 1; }
    [ -n "{{team_id}}" ] || echo "warning: unsigned build; the camera extension needs TEAM_ID=<your team> to activate"
    rm -rf /Applications/CameraCenter.app
    ditto "{{dist}}/macos/CameraCenter.app" /Applications/CameraCenter.app
    echo "Installed /Applications/CameraCenter.app — open it and click 'Install extension'."

# Remove /Applications/CameraCenter.app (deactivate the extension in System Settings first).
uninstall-osx:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "macos" ] || { echo "uninstall-osx requires a macOS host"; exit 1; }
    rm -rf /Applications/CameraCenter.app
    echo "Removed. If the extension is still listed: System Settings → General → Login Items & Extensions → Camera Extensions."

# Install under PREFIX (default ~/.local): binary, models and a desktop entry. No root needed.
install-linux: build-linux
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "linux" ] || { echo "install-linux requires a Linux host"; exit 1; }
    prefix="${PREFIX:-$HOME/.local}"
    install -Dm755 "{{dist}}/linux/camera-center" "$prefix/bin/camera-center"
    install -d "$prefix/share/camera-center/models"
    install -m644 "{{dist}}"/linux/models/* "$prefix/share/camera-center/models/"
    install -d "$prefix/share/applications"
    printf '%s\n' \
        '[Desktop Entry]' \
        'Type=Application' \
        'Name=Camera Center' \
        'Comment=Webcam effects and virtual camera' \
        "Exec=$prefix/bin/camera-center" \
        'Icon=camera-video' \
        'Categories=AudioVideo;Video;' \
        > "$prefix/share/applications/camera-center.desktop"
    echo "Installed to $prefix (make sure $prefix/bin is on PATH)."
    if ! lsmod 2>/dev/null | grep -q '^v4l2loopback'; then
        echo "The virtual camera needs v4l2loopback (run once, as root):"
        echo "  sudo modprobe v4l2loopback exclusive_caps=1 card_label=\"Camera Center\""
        echo "  # persist: echo v4l2loopback | sudo tee /etc/modules-load.d/camera-center.conf"
        echo "  #          echo 'options v4l2loopback exclusive_caps=1 card_label=\"Camera Center\"' | sudo tee /etc/modprobe.d/camera-center.conf"
    fi

# Remove a Linux install from PREFIX (default ~/.local).
uninstall-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="${PREFIX:-$HOME/.local}"
    rm -f "$prefix/bin/camera-center" "$prefix/share/applications/camera-center.desktop"
    rm -rf "$prefix/share/camera-center"
    echo "Removed Camera Center from $prefix."

# Install to %ProgramFiles%\CameraCenter and register the virtual camera source. Run from an elevated shell.
install-windows: build-windows
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "windows" ] || { echo "install-windows requires a Windows host"; exit 1; }
    net session >/dev/null 2>&1 || { echo "install-windows must run from an elevated (Administrator) shell"; exit 1; }
    # Frame Server runs as LocalService, so the DLL must live outside the user profile.
    dest="$(cygpath -u "${PROGRAMFILES:-C:\Program Files}")/CameraCenter"
    mkdir -p "$dest/models"
    cp "{{dist}}/windows/camera-center.exe" "{{dist}}/windows/cc_mf_source.dll" "$dest/"
    cp "{{dist}}"/windows/models/* "$dest/models/"
    regsvr32 //s "$(cygpath -w "$dest/cc_mf_source.dll")"
    echo "Installed to $(cygpath -w "$dest") and registered the media source."

# Unregister the media source and remove %ProgramFiles%\CameraCenter. Run from an elevated shell.
uninstall-windows:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "windows" ] || { echo "uninstall-windows requires a Windows host"; exit 1; }
    net session >/dev/null 2>&1 || { echo "uninstall-windows must run from an elevated (Administrator) shell"; exit 1; }
    dest="$(cygpath -u "${PROGRAMFILES:-C:\Program Files}")/CameraCenter"
    if [ -f "$dest/cc_mf_source.dll" ]; then
        regsvr32 //s //u "$(cygpath -w "$dest/cc_mf_source.dll")"
    fi
    rm -rf "$dest"
    echo "Removed Camera Center."

# Run the app from source (CAMERA_CENTER_SOURCE=test-pattern for a synthetic camera).
run *args:
    cargo run --release -p cc-app -- {{args}}

# Run all automated tests.
test: verify-models
    cargo test --workspace --locked

# Format the whole codebase.
fmt:
    cargo fmt --all

# All checks: formatting, lints, type-check, dependency audit, model integrity.
check: verify-models
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo check --workspace --all-targets --locked
    @command -v cargo-deny >/dev/null || { echo "cargo-deny missing: cargo install --locked cargo-deny@0.20.2"; exit 1; }
    cargo deny --locked check
    just check-cross

# Type-check the platform backends of the other OSes (skips targets that aren't installed).
check-cross:
    #!/usr/bin/env bash
    set -euo pipefail
    installed=$(rustup target list --installed)
    if grep -q x86_64-pc-windows-msvc <<<"$installed"; then
        cargo check --locked --target x86_64-pc-windows-msvc -p cc-virtualcam -p cc-mf-source -p cc-vcam-protocol
    else
        echo "skip windows cross-check (rustup target add x86_64-pc-windows-msvc)"
    fi
    if grep -q x86_64-unknown-linux-gnu <<<"$installed"; then
        cargo check --locked --target x86_64-unknown-linux-gnu -p cc-virtualcam
    else
        echo "skip linux cross-check (rustup target add x86_64-unknown-linux-gnu)"
    fi

# Verify the ONNX models against the sha256 sums in models/models.toml.
verify-models:
    #!/usr/bin/env bash
    set -euo pipefail
    cd models
    status=0
    while read -r file sha; do
        if [ ! -f "$file" ]; then echo "missing $file (git lfs pull)"; status=1; continue; fi
        actual=$( (sha256sum "$file" 2>/dev/null || shasum -a 256 "$file") | cut -d' ' -f1)
        if [ "$actual" != "$sha" ]; then echo "checksum mismatch for $file (is it a Git LFS pointer? run git lfs pull)"; status=1; fi
    done < <(awk -F'"' '/^file =/{f=$2} /^sha256 =/{print f, $2}' models.toml)
    exit $status

# Re-download the models from their pinned Hugging Face revisions and verify them.
fetch-models:
    #!/usr/bin/env bash
    set -euo pipefail
    cd models
    while read -r file url; do
        echo "fetching $file"
        curl -fsSL --proto '=https' -o "$file.partial" "$url"
        mv "$file.partial" "$file"
    done < <(awk -F'"' '/^file =/{f=$2} /^url =/{print f, $2}' models.toml)
    just verify-models
