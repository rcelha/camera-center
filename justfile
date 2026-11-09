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

# Package every platform the current host can build into dist/packages (the release workflow runs this per OS).
package: package-osx package-windows package-linux

# Package the macOS app as a drag-to-install .dmg. VERSION overrides the Cargo version in the file name.
package-osx: build-osx
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "macos" ] || { echo "skipping package-osx: requires a macOS host"; exit 0; }
    version="${VERSION:-$(cargo pkgid -p cc-app)}"; version="${version##*[#@]}"
    staging="{{target_dir}}/package/dmg"
    rm -rf "$staging" && mkdir -p "$staging" "{{dist}}/packages"
    ditto "{{dist}}/macos/CameraCenter.app" "$staging/CameraCenter.app"
    ln -s /Applications "$staging/Applications"
    out="{{dist}}/packages/camera-center-$version-macos-arm64.dmg"
    rm -f "$out"
    # hdiutil sometimes fails with "Resource busy" on CI runners; retry it.
    for attempt in 1 2 3; do
        hdiutil create -quiet -volname "Camera Center" -srcfolder "$staging" -format UDZO -ov "$out" && break
        [ "$attempt" = 3 ] && exit 1
        echo "hdiutil failed (attempt $attempt), retrying"; sleep 5
    done
    echo "packaged $out"

# Package the Windows build as a .zip with install instructions. VERSION overrides the Cargo version.
package-windows: build-windows
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "windows" ] || { echo "skipping package-windows: requires a Windows host"; exit 0; }
    version="${VERSION:-$(cargo pkgid -p cc-app)}"; version="${version##*[#@]}"
    name="camera-center-$version-windows-x64"
    staging="{{target_dir}}/package/$name"
    rm -rf "$staging" && mkdir -p "$staging" "{{dist}}/packages"
    cp -r "{{dist}}"/windows/* "$staging/"
    printf '%s\r\n' \
        'Camera Center for Windows 11 (x64)' \
        '' \
        'Install, from an elevated (Administrator) PowerShell in this folder:' \
        '' \
        '  $dest = "$env:ProgramFiles\CameraCenter"' \
        '  New-Item -ItemType Directory -Force $dest | Out-Null' \
        '  Copy-Item -Recurse -Force * $dest' \
        '  regsvr32 /s "$dest\cc_mf_source.dll"' \
        '' \
        'The DLL must live outside your user profile: the Frame Server service that' \
        'loads it runs as LocalService. Then start "$dest\camera-center.exe".' \
        '' \
        'Uninstall: regsvr32 /s /u "$env:ProgramFiles\CameraCenter\cc_mf_source.dll",' \
        'then delete the folder.' \
        > "$staging/INSTALL.txt"
    out="{{dist}}/packages/$name.zip"
    rm -f "$out"
    powershell -NoProfile -NonInteractive -Command \
        "Compress-Archive -Path '$(cygpath -w "$staging")' -DestinationPath '$(cygpath -w "$out")'"
    echo "packaged $out"

# Package the Linux build as a .deb and an AppImage.
package-linux: package-deb package-appimage

# Build the Debian package with cargo-deb. VERSION overrides the Cargo version.
package-deb: build-linux
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "linux" ] || { echo "skipping package-deb: requires a Linux host"; exit 0; }
    command -v cargo-deb >/dev/null || { echo "cargo-deb missing: cargo install --locked cargo-deb@3.8.0"; exit 1; }
    version="${VERSION:-$(cargo pkgid -p cc-app)}"; version="${version##*[#@]}"
    mkdir -p "{{dist}}/packages"
    out="{{dist}}/packages/camera-center_${version}_amd64.deb"
    cargo deb -p cc-app --target x86_64-unknown-linux-gnu --no-build --deb-version "$version" -o "$out"
    echo "packaged $out"

# Build the x86_64 AppImage with linuxdeploy (downloaded at a pinned version and checksum). VERSION overrides the Cargo version.
package-appimage:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{host}}" = "linux" ] || { echo "skipping package-appimage: requires a Linux host"; exit 0; }
    [ "$(uname -m)" = "x86_64" ] || { echo "package-appimage requires an x86_64 host"; exit 1; }
    version="${VERSION:-$(cargo pkgid -p cc-app)}"; version="${version##*[#@]}"

    # Pinned packaging tools, verified before use. Bump the URL and the sha256 together.
    tools="{{target_dir}}/package/tools"
    mkdir -p "$tools"
    fetch() { # name url sha256
        local dest="$tools/$1"
        if [ -f "$dest" ] && echo "$3  $dest" | sha256sum -c --status; then return; fi
        curl -fsSL --retry 3 --retry-all-errors --proto '=https' -o "$dest.partial" "$2"
        echo "$3  $dest.partial" | sha256sum -c --quiet || { echo "checksum mismatch for $1"; rm -f "$dest.partial"; exit 1; }
        mv "$dest.partial" "$dest" && chmod +x "$dest"
    }
    fetch linuxdeploy-x86_64.AppImage \
        https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage \
        c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
    fetch linuxdeploy-plugin-gtk.sh \
        https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/7a3fbc31a9e5075073ff8790f26effbac5f84453/linuxdeploy-plugin-gtk.sh \
        b0f4cbc684a0103a9651f0955b635eaea0096b3a66c0f5a2c2aa337960375171
    fetch onnxruntime-linux-x64-1.31.1.tgz \
        https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-linux-x64-1.31.1.tgz \
        c849e36a1ef3e22d4ff217b9d1ce4a3cf5b9467b975610913ae959ad5b16bdc9

    # ort's default prebuilt static ONNX Runtime needs glibc 2.39. Link the official shared build instead
    # (glibc 2.28) and bundle it, so the AppImage runs on older distros. Built in its own target dir so the
    # .deb binary (static ONNX Runtime) is left untouched.
    ort="{{target_dir}}/package/onnxruntime"
    rm -rf "$ort" && mkdir -p "$ort" && tar -xzf "$tools/onnxruntime-linux-x64-1.31.1.tgz" -C "$ort" --strip-components=1
    build="{{target_dir}}/appimage-build"
    ORT_LIB_LOCATION="$ort/lib" ORT_PREFER_DYNAMIC_LINK=1 CARGO_TARGET_DIR="$build" \
        cargo build --release --locked --target x86_64-unknown-linux-gnu -p cc-app

    # Libraries loaded with dlopen are invisible to linuxdeploy's ldd scan, so list them explicitly:
    # the tray icon (ayatana-appindicator) and winit's keyboard/cursor/X11 extension libraries.
    # Graphics drivers, glibc, X11/Wayland core and fontconfig stay on the host (linuxdeploy excludelist).
    libs=(--library "$ort/lib/libonnxruntime.so.1")
    for lib in libayatana-appindicator3.so.1 libxkbcommon.so.0 libxkbcommon-x11.so.0 libXcursor.so.1 \
               libXrandr.so.2 libXi.so.6 libXinerama.so.1 libwayland-cursor.so.0; do
        path=$(/sbin/ldconfig -p | awk -v l="$lib" '$1 == l && /x86-64/ && !p {p = $NF} END {print p}')
        [ -n "$path" ] || { echo "$lib not found (see the Linux system dependencies in the README)"; exit 1; }
        libs+=(--library "$path")
    done

    work="{{target_dir}}/package/appimage"
    appdir="$work/AppDir"
    rm -rf "$work" && mkdir -p "$appdir/usr/share/camera-center/models" "{{dist}}/packages"
    install -Dm755 "$build/x86_64-unknown-linux-gnu/release/camera-center" "$appdir/usr/bin/camera-center"
    install -m644 models/models.toml models/*.onnx "$appdir/usr/share/camera-center/models/"
    install -m644 crates/cc-app/assets/tray.png "$work/camera-center.png"

    out="{{dist}}/packages/camera-center-$version-x86_64.AppImage"
    rm -f "$out"
    # APPIMAGE_EXTRACT_AND_RUN: run the tool AppImages without FUSE (CI runners and containers lack it).
    PATH="$tools:$PATH" APPIMAGE_EXTRACT_AND_RUN=1 DEPLOY_GTK_VERSION=3 \
        LD_LIBRARY_PATH="$ort/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
        LDAI_OUTPUT="$out" LINUXDEPLOY_OUTPUT_VERSION="$version" \
        linuxdeploy-x86_64.AppImage \
            --appdir "$appdir" \
            --executable "$appdir/usr/bin/camera-center" \
            --desktop-file platform/linux/camera-center.desktop \
            --icon-file "$work/camera-center.png" \
            "${libs[@]}" \
            --plugin gtk \
            --output appimage
    echo "packaged $out"

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
check: deny lint

# Formatting, lints and type-checks (host + cross targets). CI runs this per OS and `deny` once.
lint: verify-models
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo check --workspace --all-targets --locked
    just check-cross

# Dependency audit: advisories, licenses, bans, sources (no compilation needed, so it runs first).
deny:
    @command -v cargo-deny >/dev/null || { echo "cargo-deny missing: cargo install --locked cargo-deny@0.20.2"; exit 1; }
    cargo deny --locked check

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

# Re-create the MediaPipe models (`tflite_url` in models/models.toml) by converting
# Google's TFLite files to ONNX with a pinned toolchain (needs uv), then verify them.
convert-models:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v uv >/dev/null || { echo "uv missing: https://docs.astral.sh/uv/"; exit 1; }
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    uv venv -q -p 3.11 "$work/venv"
    VIRTUAL_ENV="$work/venv" uv pip sync -q --require-hashes models/convert-requirements.txt
    mkdir "$work/tflite"
    while read -r file url sha; do
        src="$work/tflite/${file%.onnx}.tflite"
        echo "converting $url"
        curl -fsSL --proto '=https' -o "$src" "$url"
        actual=$( (sha256sum "$src" 2>/dev/null || shasum -a 256 "$src") | cut -d' ' -f1)
        if [ "$actual" != "$sha" ]; then echo "checksum mismatch for $url"; exit 1; fi
        "$work/venv/bin/python" -I models/convert_mediapipe.py "$src" "models/$file"
    done < <(awk -F'"' '/^file =/{f=$2} /^tflite_url =/{u=$2} /^tflite_sha256 =/{print f, u, $2}' models/models.toml)
    just verify-models
