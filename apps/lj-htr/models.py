# /// script
# requires-python = "==3.10.*"
# dependencies = ["pylaia==1.1.2", "setuptools<70", "onnx==1.17.0", "numpy<2"]
# ///
"""The three networks as the module reads them: fp16 weights for the detector
and the line reader, int8 for the handwriting reader, every graph in fp32."""

import inspect
import shutil
import sys
from pathlib import Path

import numpy as np
import onnx
import torch
from laia.models.htr.laia_crnn import LaiaCRNN
from onnx import helper, numpy_helper

LARGE = 4096


def plain(model: onnx.ModelProto) -> onnx.ModelProto:
    """tract refuses a dotted symbolic dimension, and the shapes inferred for one."""
    for value in list(model.graph.input) + list(model.graph.output):
        for dim in value.type.tensor_type.shape.dim:
            if dim.dim_param:
                dim.dim_param = dim.dim_param.replace(".", "_")
    del model.graph.value_info[:]
    return model


def half(model: onnx.ModelProto) -> onnx.ModelProto:
    def narrow(tensor: onnx.TensorProto) -> None:
        if tensor.data_type == onnx.TensorProto.FLOAT:
            array = numpy_helper.to_array(tensor)
            if array.size >= 256:
                tensor.CopyFrom(numpy_helper.from_array(array.astype(np.float16), tensor.name))

    for tensor in model.graph.initializer:
        narrow(tensor)
    for node in model.graph.node:
        if node.op_type == "Constant":
            for attribute in node.attribute:
                if attribute.name == "value":
                    narrow(attribute.t)
    return model


def quarter(model: onnx.ModelProto) -> onnx.ModelProto:
    """Symmetric int8 per output channel: a convolution's, an LSTM gate's, a MatMul's."""
    kept, nodes = [], []
    for tensor in model.graph.initializer:
        array = numpy_helper.to_array(tensor)
        if tensor.data_type != onnx.TensorProto.FLOAT or array.size < LARGE or array.ndim < 2:
            kept.append(tensor)
            continue
        axis = {4: 0, 3: 1, 2: 1}[array.ndim]
        others = tuple(i for i in range(array.ndim) if i != axis)
        scale = np.abs(array).max(others) / 127
        scale[scale == 0] = 1
        shape = [1] * array.ndim
        shape[axis] = -1
        q = np.clip(np.round(array / scale.reshape(shape)), -127, 127).astype(np.int8)
        name = tensor.name
        kept += [
            numpy_helper.from_array(q, name + "_q"),
            numpy_helper.from_array(scale.astype(np.float32), name + "_s"),
            numpy_helper.from_array(np.zeros_like(scale, np.int8), name + "_z"),
        ]
        nodes.append(helper.make_node("DequantizeLinear", [name + "_q", name + "_s", name + "_z"], [name], axis=axis))
    del model.graph.initializer[:]
    model.graph.initializer.extend(kept)
    graph = list(model.graph.node)
    del model.graph.node[:]
    model.graph.node.extend(nodes + graph)
    for opset in model.opset_import:
        if opset.domain in ("", "ai.onnx"):
            opset.version = max(opset.version, 13)
    return model


class Softmax(torch.nn.Module):
    def __init__(self, net: torch.nn.Module) -> None:
        super().__init__()
        self.net = net

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return torch.softmax(self.net(x), -1)


def laia(downloads: Path, out: Path) -> None:
    spec = torch.load(downloads / "model", weights_only=False)
    known = inspect.signature(LaiaCRNN.__init__).parameters
    net = LaiaCRNN(**{k: v for k, v in spec["kwargs"].items() if k in known})
    weights = torch.load(downloads / "weights.ckpt", map_location="cpu", weights_only=False)["state_dict"]
    net.load_state_dict({k.removeprefix("model."): v for k, v in weights.items()})
    net.eval()
    torch.onnx.export(
        Softmax(net),
        torch.rand(1, 1, 128, 1024),
        out,
        input_names=["x"],
        output_names=["p"],
        dynamic_axes={"x": {3: "W"}, "p": {0: "T"}},
        opset_version=14,
    )


def main() -> None:
    downloads, site = Path(sys.argv[1]), Path(sys.argv[2])
    site.mkdir(parents=True, exist_ok=True)
    onnx.save(half(plain(onnx.load(downloads / "det.onnx"))), site / "det.onnx")
    onnx.save(half(plain(onnx.load(downloads / "rec.onnx"))), site / "rec.onnx")
    wide = downloads / "laia32.onnx"
    laia(downloads, wide)
    onnx.save(quarter(plain(onnx.load(wide))), site / "laia.onnx")
    wide.unlink()
    for name in ["dict.txt", "syms.txt"] + [p.name for p in downloads.glob("lexicon-*.txt")]:
        shutil.copy(downloads / name, site / name)


if __name__ == "__main__":
    main()
