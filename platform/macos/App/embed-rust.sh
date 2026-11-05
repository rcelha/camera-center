#!/bin/sh
# Xcode build phase: replace the stub executable with the Rust binary and
# bundle the segmentation models. Runs before Xcode signs the app.
set -eu

: "${RUST_BINARY:?set RUST_BINARY to the cargo-built camera-center binary (see justfile)}"
: "${MODELS_DIR:?set MODELS_DIR to the repository models directory}"

if [ ! -x "$RUST_BINARY" ]; then
    echo "error: $RUST_BINARY not found; run 'cargo build --release -p cc-app' first" >&2
    exit 1
fi

install -m 755 "$RUST_BINARY" "$TARGET_BUILD_DIR/$EXECUTABLE_PATH"

RESOURCES="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/models"
mkdir -p "$RESOURCES"
cp "$MODELS_DIR/models.toml" "$RESOURCES/"
for model in "$MODELS_DIR"/*.onnx; do
    cp "$model" "$RESOURCES/"
done
