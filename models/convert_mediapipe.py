"""Convert a MediaPipe selfie segmenter TFLite model to ONNX.

    python convert_mediapipe.py <in.tflite> <out.onnx>

Run it through `just convert-models`, which installs the pinned toolchain from
convert-requirements.txt and checks the result against models.toml.

tf2onnx converts every op except MediaPipe's custom
`Convolution2DTransposeBias` (the final 2x upsampling layer), which it leaves
as an opaque node. That node is replaced with a standard ONNX `ConvTranspose`
using the kernel and bias tf2onnx already extracted. The custom op's options
are checked first (SAME padding, stride 2, 2x2 kernel, which needs no padding).

The output is byte-reproducible: tf2onnx's optimiser and naming vary between
runs, so the graph is passed through onnx-simplifier and every tensor is
renamed by topological position. The graph input is `input` (NHWC float RGB
in [0, 1]) and the output is `mask` (NHWC person probability).

Finally the ONNX model is run next to MediaPipe's own TFLite runtime, which
implements the custom op, and the outputs must match.
"""

import hashlib
import os
import struct
import subprocess
import sys
import tempfile

import numpy as np
import onnx
import onnxruntime
import onnxsim
from onnx import checker, helper, numpy_helper

CUSTOM_OP = "Convolution2DTransposeBias"


def custom_op_options(tflite_path):
    """(padding, stride_w, stride_h) of the single custom op in the model."""
    from tf2onnx.tflite import Model

    buf = open(tflite_path, "rb").read()
    model = Model.Model.GetRootAsModel(buf, 0)
    graph = model.Subgraphs(0)
    found = []
    for i in range(graph.OperatorsLength()):
        op = graph.Operators(i)
        code = model.OperatorCodes(op.OpcodeIndex()).CustomCode()
        if code:
            found.append((code.decode(), bytes(op.CustomOptionsAsNumpy())))
    assert [c for c, _ in found] == [CUSTOM_OP], f"unexpected custom ops {found}"
    # MediaPipe stores TfLiteTransposeConvParams as raw little-endian ints.
    return struct.unpack("<3i", found[0][1][:12])


def tf2onnx_convert(tflite_path):
    with tempfile.TemporaryDirectory() as tmp:
        raw = os.path.join(tmp, "raw.onnx")
        subprocess.run(
            [sys.executable, "-I", "-m", "tf2onnx.convert", "--tflite", tflite_path,
             "--output", raw, "--opset", "17"],
            check=True,
            env={**os.environ, "PYTHONHASHSEED": "0"},
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return onnx.load(raw)


def replace_custom_op(model):
    g = model.graph
    inits = {t.name: t for t in g.initializer}
    (idx, node), = [(i, n) for i, n in enumerate(g.node) if n.op_type == f"TFL_{CUSTOM_OP}"]
    x_nhwc, w_name, b_name = node.input
    (producer,) = [n for n in g.node if x_nhwc in n.output]
    assert producer.op_type == "Transpose" and list(producer.attribute[0].ints) == [0, 2, 3, 1]
    x_nchw = producer.input[0]

    w = numpy_helper.to_array(inits[w_name])  # TFLite OHWI, (1, 2, 2, C)
    b = numpy_helper.to_array(inits[b_name])  # (1,)
    assert w.shape[:3] == (1, 2, 2) and b.shape == (1,), (w.shape, b.shape)
    w = np.ascontiguousarray(w.transpose(3, 0, 1, 2)).astype(np.float32)  # (Cin, Cout, kH, kW)
    g.initializer.append(numpy_helper.from_array(w, "upsample_W"))
    g.initializer.append(numpy_helper.from_array(b.astype(np.float32), "upsample_B"))

    out = node.output[0]
    deconv = helper.make_node(
        "ConvTranspose", [x_nchw, "upsample_W", "upsample_B"], [out + "_nchw"],
        kernel_shape=[2, 2], strides=[2, 2], pads=[0, 0, 0, 0],
    )
    to_nhwc = helper.make_node("Transpose", [out + "_nchw"], [out], perm=[0, 2, 3, 1])
    del g.node[idx]
    g.node.insert(idx, to_nhwc)
    g.node.insert(idx, deconv)

    used = {x for n in g.node for x in n.input} | {o.name for o in g.output}
    if not any(o in used for o in producer.output):
        g.node.remove(producer)
    used = {x for n in g.node for x in n.input}
    for t in [t for t in g.initializer if t.name not in used]:
        g.initializer.remove(t)
    keep = [o for o in model.opset_import if o.domain in ("", "ai.onnx")]
    del model.opset_import[:]
    model.opset_import.extend(keep)


def use_global_pools(model):
    """Rewrites full-window AveragePool (squeeze-excite blocks) as GlobalAveragePool.

    Same result, but ONNX Runtime's CoreML MLProgram builder rejects these
    AveragePool nodes ("Required param 'pad' is missing").
    """
    shapes = {
        v.name: [d.dim_value for d in v.type.tensor_type.shape.dim]
        for v in onnx.shape_inference.infer_shapes(model).graph.value_info
    }
    for n in model.graph.node:
        if n.op_type != "AveragePool":
            continue
        attrs = {a.name: onnx.helper.get_attribute_value(a) for a in n.attribute}
        spatial = shapes[n.input[0]][2:]
        assert attrs.get("kernel_shape") == spatial and not any(attrs.get("pads", [])), attrs
        n.op_type = "GlobalAveragePool"
        del n.attribute[:]


def explicit_pads(model):
    """Spells out the default all-zero `pads` on Conv nodes.

    ONNX defaults a missing `pads` to zeros, but ONNX Runtime's CoreML
    MLProgram builder fails with "Required param 'pad' is missing".
    """
    for n in model.graph.node:
        if n.op_type not in ("Conv", "ConvTranspose"):
            continue
        names = {a.name for a in n.attribute}
        if "pads" in names or "auto_pad" in names:
            continue
        kernel = next(a.ints for a in n.attribute if a.name == "kernel_shape")
        n.attribute.append(helper.make_attribute("pads", [0] * (2 * len(kernel))))


def canonicalise(model, name):
    model, ok = onnxsim.simplify(model)
    assert ok, "onnx-simplifier could not validate the simplified model"
    use_global_pools(model)
    explicit_pads(model)
    g = model.graph
    rename = {g.input[0].name: "input", g.output[0].name: "mask"}
    for i, n in enumerate(g.node):
        for x in n.input:
            if x and x not in rename:
                rename[x] = f"w{len(rename)}"
        for x in n.output:
            if x not in rename:
                rename[x] = f"t{len(rename)}"
        n.name = f"n{i}_{n.op_type}"
    for n in g.node:
        n.input[:] = [rename.get(x, x) for x in n.input]
        n.output[:] = [rename.get(x, x) for x in n.output]
    for v in list(g.input) + list(g.output) + list(g.value_info):
        v.name = rename.get(v.name, v.name)
    for t in g.initializer:
        t.name = rename[t.name]
    ordered = sorted(g.initializer, key=lambda t: int(t.name[1:]))
    del g.initializer[:]
    g.initializer.extend(ordered)
    g.name = name
    g.doc_string = ""  # tf2onnx records the (temporary) source path here
    model.producer_name = "camera-center models/convert_mediapipe.py"
    model.producer_version = ""
    # ONNX Runtime's CoreML EP keys its compiled-model cache on the file path
    # unless this is set, so a re-converted model would load a stale build.
    key = hashlib.sha256(model.SerializeToString()).hexdigest()[:32]
    model.metadata_props.add(key="COREML_CACHE_KEY", value=key)
    return model


def check_against_mediapipe(tflite_path, onnx_path):
    import mediapipe as mp
    from mediapipe.tasks import python as mpt
    from mediapipe.tasks.python import vision

    sess = onnxruntime.InferenceSession(onnx_path, providers=["CPUExecutionProvider"])
    _, h, w, _ = sess.get_inputs()[0].shape
    segmenter = vision.ImageSegmenter.create_from_options(vision.ImageSegmenterOptions(
        base_options=mpt.BaseOptions(model_asset_path=tflite_path),
        output_confidence_masks=True,
    ))
    rng = np.random.default_rng(0)
    yy, xx = np.mgrid[0:h, 0:w]
    blob = ((xx - w / 2) ** 2 / (w / 4) ** 2 + (yy - h / 2) ** 2 / (h / 3) ** 2 < 1)
    images = [
        rng.integers(0, 256, (h, w, 3), dtype=np.uint8),
        np.where(blob[..., None], [225, 180, 150], [40, 60, 70]).astype(np.uint8),
    ]
    worst = 0.0
    for img in images:
        ours = sess.run(None, {"input": img[None].astype(np.float32) / 255.0})[0][0, :, :, 0]
        ref = segmenter.segment(mp.Image(image_format=mp.ImageFormat.SRGB, data=img))
        ref = np.squeeze(ref.confidence_masks[-1].numpy_view())
        worst = max(worst, float(np.abs(ours - ref).max()))
    # 1e-3 is a quarter of one 8-bit mask level; float summation order differs.
    assert worst < 1e-3, f"ONNX output differs from MediaPipe by {worst}"
    return worst


def main():
    src, dst = sys.argv[1:3]
    padding, stride_w, stride_h = custom_op_options(src)
    assert (padding, stride_w, stride_h) == (1, 2, 2), (padding, stride_w, stride_h)  # 1 = SAME

    model = tf2onnx_convert(src)
    replace_custom_op(model)
    model = canonicalise(model, os.path.splitext(os.path.basename(dst))[0])
    checker.check_model(model, full_check=True)
    onnx.save(model, dst)

    worst = check_against_mediapipe(src, dst)
    shape = [d.dim_value for d in model.graph.input[0].type.tensor_type.shape.dim]
    print(f"{dst}: input {shape}, max |onnx - mediapipe| = {worst:.2e}")


if __name__ == "__main__":
    main()
