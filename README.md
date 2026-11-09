# Camera Center

Camera Center captures your webcam, runs the video through an effects
pipeline, and publishes the result as a **virtual camera** ("Camera Center")
that Zoom, Meet, Teams, FaceTime or OBS can use.

Effects, each with its own toggle:

- **Blur background**: Gaussian blur behind you, strength set in Settings.
- **Background image**: replaces the background with an image you choose.
  If blur is also on, the *image* is blurred instead. It is blurred once and
  cached, not every frame.
- **Portrait mode**: shallow depth of field, subject exposure and tone,
  vignette.

The UI shows the original and processed feeds side by side, the virtual camera
name and status, and a Settings page with: input device, processing resolution,
segmentation model, background image and blur strength.

Closing the window doesn't quit. Camera Center keeps processing and feeding the
virtual camera from the system tray (the menu bar on macOS).
- **Open Camera Center** (or left-click the icon on Windows/Linux) reopens the window.
- **Quit Camera Center** stops everything.

On Linux the tray needs an AppIndicator host: install `libayatana-appindicator3`,
and on GNOME the AppIndicator extension.

## Architecture

```
capture thread ──Latest<Frame>──▶ processing thread ──▶ virtual camera (per OS)
                                   │      ▲                 └─▶ UI previews (Freya)
                      Latest<Frame>│      │ArcSwap<Mask>
                                   ▼      │
                             segmentation thread (ONNX Runtime, MediaPipe selfie / U²-Net)
```

- Each hand-off keeps only the newest item. A slow stage drops frames instead
  of adding latency.
- Segmentation runs asynchronously, and the compositor always uses the newest
  mask, smoothed over time to reduce flicker.
- Pipeline order is fixed: `BackgroundImage → BlurBackground → Composite → Portrait`.

| Crate | Purpose |
|-------|---------|
| `cc-core` | Frame/Mask types, config, blur, resize, pixel conversion, effects, `Pipeline` |
| `cc-ml` | ONNX Runtime person segmenters, MediaPipe selfie and U²-Net (CoreML / DirectML / CPU) |
| `cc-capture` | Camera enumeration and capture. macOS uses AVFoundation directly; Linux/Windows use nokhwa |
| `cc-virtualcam` | `VirtualCamera` trait plus the v4l2loopback, CoreMediaIO sink and Media Foundation backends |
| `cc-vcam-protocol` | Shared-memory frame protocol between the Windows app and its media source |
| `cc-mf-source` | Windows Media Foundation virtual camera source (COM DLL loaded by Frame Server) |
| `cc-engine` | Thread wiring, status, test-pattern source |
| `cc-app` | Freya desktop UI (`camera-center` binary) |
| `platform/macos` | CoreMediaIO Camera Extension (Swift) and the Xcode project that bundles `CameraCenter.app` |

## Getting started

```sh
git lfs install && git lfs pull   # fetch the ONNX models
just verify-models
just run                          # run from source (camera)
CAMERA_CENTER_SOURCE=test-pattern just run   # synthetic camera
```

| Recipe | What it does |
|--------|--------------|
| `just build` | Builds every platform the host can build. Others are skipped; CI builds all three. |
| `just build-osx` / `build-windows` / `build-linux` | Builds that platform's package into `dist/<platform>` |
| `just package` / `package-osx` / `package-windows` / `package-linux` | Builds the release packages into `dist/packages` (see [Releases](#releases)) |
| `just install` / `just uninstall` | Installs or removes the app for the current host |
| `just install-osx` / `install-windows` / `install-linux` | Installs that platform's package (see below) |
| `just test` | Runs all tests, including real inference on every model |
| `just fmt` | Runs `cargo fmt` |
| `just check` | `just deny` + `just lint` |
| `just deny` | `cargo deny` dependency audit (advisories, licenses, bans, sources) |
| `just lint` | fmt check, clippy `-D warnings`, `cargo check`, model checksums, cross-target type-check |
| `just fetch-models` / `just verify-models` | Re-downloads the U²-Net models from their pinned revisions, or verifies every model's SHA-256 |
| `just convert-models` | Rebuilds the MediaPipe ONNX models from Google's TFLite files (needs `uv`, see [Models](#models)) |

`just check` requires `cargo install --locked cargo-deny@0.20.2`. Accepted advisories (unmaintained transitive crates with no upgrade path) are listed with reasons in `deny.toml`; security advisories are never ignored.

Settings are stored as TOML in the platform config directory. On macOS that's
`~/Library/Application Support/com.cameracenter.Camera-Center/config.toml`.
Set `CAMERA_CENTER_CONFIG` to use another file.

## Releases

Releases are automated with [release-plz](https://release-plz.dev) (`release-plz.toml`).
Nothing is published to crates.io. Every crate shares the workspace version,
and the latest `vX.Y.Z` tag is the previous release.

1. Each push to `main` opens or updates a **release PR**. It bumps the version
   in `Cargo.toml` and adds the new commits to `CHANGELOG.md`. Use
   [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`,
   `feat!:`) to choose the bump; other messages count as a patch and go under
   "Other". Only changes inside `crates/` count, so a change that touches only
   `platform/`, `models/` or the `justfile` doesn't open a release PR by itself.
2. Merging the release PR tags `vX.Y.Z` and creates a **draft** GitHub Release.
3. `.github/workflows/release-build.yml` builds the packages on each OS, attaches
   them with a `SHA256SUMS` file, and publishes the release:

| Platform | Package |
|----------|---------|
| macOS (Apple Silicon) | `camera-center-<v>-macos-arm64.dmg` |
| Windows (x64) | `camera-center-<v>-windows-x64.zip` (with `INSTALL.txt`) |
| Linux (x64) | `camera-center_<v>_amd64.deb`, `camera-center-<v>-x86_64.AppImage` |

The Linux packages are built on Ubuntu 24.04 and need glibc 2.39 or newer
(Ubuntu 24.04+, Debian 13+, Fedora 40+). The prebuilt ONNX Runtime needs
glibc 2.38 or newer, so they can't be built on an older base.
To rebuild the packages for an existing tag, run the *Release packages* workflow
manually. Pull requests that change packaging (`justfile`, `platform/`,
`crates/cc-app/Cargo.toml`, the workflow) build the packages as workflow
artifacts but don't publish anything.

Repository setup: add a `RELEASE_PLZ_TOKEN` secret, a fine-grained PAT or GitHub
App token with *Contents* and *Pull requests* write access. Without it,
release-plz uses `GITHUB_TOKEN`, so CI doesn't run on the release PR. You then
also need to enable *Settings → Actions → Allow GitHub Actions to create and
approve pull requests*.

The macOS package is **not signed or notarized** yet. Gatekeeper blocks it after
download (`xattr -dr com.apple.quarantine /Applications/CameraCenter.app`
works around that), and the camera extension won't activate without a
Developer ID signature. Signing in CI needs the team's certificate and
provisioning profiles as secrets.

## Models

Models are versioned in `models/` with Git LFS. `models/models.toml` records
each file's source, pinned revision or URL, and SHA-256.

| Model | Size | Input | Notes |
|-------|------|-------|-------|
| `selfie_segmenter` | 0.4 MB | 256×256 | Default. MediaPipe selfie segmentation. Fast enough for CPU-only machines |
| `selfie_segmenter_landscape` | 0.4 MB | 256×144 | Same network at 16:9. Fastest, slightly coarser |
| `u2netp` | 4.6 MB | 320×320 | U²-Net small |
| `u2net_human_seg` | 176 MB | 320×320 | Cleanest edges around hair, but slow without an accelerator |

Switch models under Settings → Segmentation. **Processing resolution**
(Native / 1080p / 720p / 480p) caps capture and effect resolution, so
powerful machines can process at native resolution and slower ones can force
720p. The virtual camera always outputs 1920×1080 and letterboxes when needed.

Per-inference latency on an M3 Max (`cargo run -p cc-ml --release --example bench`).
Linux has no accelerator, so it gets the CPU column:

| Model | CPU (4 threads) | CoreML (Neural Engine) |
|-------|-----------------|------------------------|
| `selfie_segmenter` | 2.3 ms | 3.1 ms |
| `selfie_segmenter_landscape` | 1.5 ms | 2.3 ms |
| `u2netp` | 135 ms | 33 ms |
| `u2net_human_seg` | 355 ms | 75 ms |

Segmentation is asynchronous, so video stays at the camera's frame rate and a
slow model only makes the mask lag. On CPU, though, U²-Net keeps several cores
busy, which competes with capture and effects.

### MediaPipe models (converted)

Google publishes the selfie segmenter only as TFLite (Apache-2.0,
[model card](https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Selfie%20Segmentation.pdf)).
`models/convert_mediapipe.py` converts it with tf2onnx and replaces MediaPipe's
one custom op, `Convolution2DTransposeBias` (the final 2× upsample), with a
standard `ConvTranspose`. It also rewrites a few nodes so the ONNX Runtime
CoreML builder accepts them, and checks the result against MediaPipe's own
runtime (max difference ≈1e-4 on a 0–1 probability). `just convert-models`
downloads the pinned TFLite files, checks their SHA-256, converts them with
the hash-locked toolchain in `models/convert-requirements.txt`, and verifies
the output is byte-identical to the committed files.

The model takes NHWC RGB in [0, 1] and returns a person probability. It is
used as is, without U²-Net's min-max stretch, so an empty room stays background.

## Platform setup

### macOS (Tahoe 26+, Apple Silicon)

The virtual camera is a CoreMediaIO **Camera Extension**. It must be signed
and run from `/Applications`:

```sh
TEAM_ID=<your Apple team id> just install-osx
```

Then open `/Applications/CameraCenter.app`, click **Install extension**, and
approve it under System Settings → General → Login Items & Extensions →
Camera Extensions. Your team ID is shown in Xcode → Settings → Accounts. Xcode
must be signed in to that account so `-allowProvisioningUpdates` can create the
profiles. The team needs the System Extension capability, which free personal
teams may not have.

macOS refuses to install the extension from `just run`, from an unsigned build,
or from outside `/Applications`. The error is "Missing entitlement
com.apple.developer.system-extension.install". In those cases the app hides the
button and shows how to install instead.

Remove it with `just uninstall-osx`. Deactivate the extension in System
Settings first.

### Linux

`just install-linux` installs to `~/.local` (override with `PREFIX=...`):
the binary, the models under `share/camera-center/models`, and a desktop
entry. No root is needed.

The virtual camera uses v4l2loopback, which you load yourself:

```sh
sudo modprobe v4l2loopback exclusive_caps=1 card_label="Camera Center"
```

### Windows (11+)

Run `just install-windows` from an **elevated** Git Bash. It copies the app to
`%ProgramFiles%\CameraCenter` and registers the media source with
`regsvr32`. The DLL has to live outside your user profile because the Frame
Server service, which loads it, runs as LocalService.

While Camera Center is running it registers a session-lifetime Media
Foundation virtual camera. Remove everything with `just uninstall-windows`.

## Status

| Component | State |
|-----------|-------|
| Pipeline, effects, config, models, engine | Unit and integration tests pass on macOS |
| macOS capture (AVFoundation) and UI | Run against a FaceTime HD camera and an AnkerWork C310 |
| macOS camera extension | Builds unsigned. Activation and streaming not yet verified (needs a signing team) |
| Linux v4l2loopback backend | Type-checked for `x86_64-unknown-linux-gnu`; not run on Linux yet |
| Windows Media Foundation source and app backend | Type-checked for `x86_64-pc-windows-msvc`; not run on Windows yet |
| Install recipes for Linux and Windows | Written, not run (need those hosts) |
