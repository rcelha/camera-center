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
                             segmentation thread (ONNX Runtime, U²-Net)
```

- Each hand-off keeps only the newest item. A slow stage drops frames instead
  of adding latency.
- Segmentation runs asynchronously, and the compositor always uses the newest
  mask, smoothed over time to reduce flicker.
- Pipeline order is fixed: `BackgroundImage → BlurBackground → Composite → Portrait`.

| Crate | Purpose |
|-------|---------|
| `cc-core` | Frame/Mask types, config, blur, resize, pixel conversion, effects, `Pipeline` |
| `cc-ml` | ONNX Runtime U²-Net segmenter (CoreML / DirectML / CPU) |
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
| `just install` / `just uninstall` | Installs or removes the app for the current host |
| `just install-osx` / `install-windows` / `install-linux` | Installs that platform's package (see below) |
| `just test` | Runs all tests, including real inference on both models |
| `just fmt` | Runs `cargo fmt` |
| `just check` | fmt check, clippy `-D warnings`, `cargo check`, `cargo deny`, model checksums, cross-target type-check |
| `just fetch-models` / `just verify-models` | Re-downloads the models from their pinned revisions, or verifies their SHA-256 |

`just check` requires `cargo install --locked cargo-deny@0.20.2`.

Settings are stored as TOML in the platform config directory. On macOS that's
`~/Library/Application Support/com.cameracenter.Camera-Center/config.toml`.
Set `CAMERA_CENTER_CONFIG` to use another file.

## Models

Models are versioned in `models/` with Git LFS. `models/models.toml` records
each file's source repository, pinned revision, URL and SHA-256.

| Model | Size | Notes |
|-------|------|-------|
| `u2net_human_seg` | 176 MB | Default. Trained for people, so it gives cleaner edges |
| `u2netp` | 4.6 MB | Faster, for low-end machines |

Switch models under Settings → Segmentation. **Processing resolution**
(Native / 1080p / 720p / 480p) caps capture and effect resolution, so
powerful machines can process at native resolution and slower ones can force
720p. The virtual camera always outputs 1920×1080 and letterboxes when needed.

Measured on an M3 Max, per 320×320 inference with CoreML on the Neural Engine:
about 44 ms for `u2netp` and 96 ms for `u2net_human_seg`. Because
segmentation is asynchronous, video stays at the camera's frame rate either
way.

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
