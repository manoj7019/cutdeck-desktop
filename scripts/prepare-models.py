"""Rebuild the background-removal models in src-tauri/resources/models/.

The repository already contains the prepared files; run this only to regenerate them.
U²-Net weights come from the rembg project (Apache-2.0):
  https://github.com/danielgatis/rembg/releases/tag/v0.0.0

What it does: freezes the input to 1x3x320x320, constant-folds the graph with onnxsim,
and rewrites every Resize node to use explicit scales with half_pixel coordinates. The
result is numerically the same model with a static graph that loads fast everywhere.

  pip install onnx onnxsim numpy
  python scripts/prepare-models.py
"""
import pathlib
import urllib.request

import numpy as np
import onnx
import onnxsim
from onnx import numpy_helper, shape_inference

OUT = pathlib.Path(__file__).resolve().parent.parent / "src-tauri" / "resources" / "models"
URL = "https://github.com/danielgatis/rembg/releases/download/v0.0.0/{}.onnx"


def prepare(name: str) -> None:
    raw = OUT / f"{name}.orig.onnx"
    if not raw.exists():
        print("downloading", name)
        urllib.request.urlretrieve(URL.format(name), raw)
    model, ok = onnxsim.simplify(onnx.load(raw), overwrite_input_shapes={"input.1": [1, 3, 320, 320]})
    assert ok, "simplify failed"
    inferred = shape_inference.infer_shapes(model)
    shapes = {
        v.name: [d.dim_value for d in v.type.tensor_type.shape.dim]
        for v in list(inferred.graph.value_info) + list(inferred.graph.input)
    }
    inits = {i.name: numpy_helper.to_array(i) for i in model.graph.initializer}
    model.graph.initializer.append(numpy_helper.from_array(np.zeros((0,), np.float32), "cd_roi"))
    for k, node in enumerate(n for n in model.graph.node if n.op_type == "Resize"):
        x = node.input[0]
        size = inits[node.input[3]]
        scales = np.array([size[i] / shapes[x][i] for i in range(4)], np.float32)
        model.graph.initializer.append(numpy_helper.from_array(scales, f"cd_scales_{k}"))
        del node.input[:]
        node.input.extend([x, "cd_roi", f"cd_scales_{k}"])
        for a in node.attribute:
            if a.name == "coordinate_transformation_mode":
                a.s = b"half_pixel"
    onnx.checker.check_model(model)
    onnx.save(model, OUT / f"{name}.onnx")
    raw.unlink()
    print("wrote", OUT / f"{name}.onnx")


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    for n in ("u2netp", "silueta"):
        prepare(n)
