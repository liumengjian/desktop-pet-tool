#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""去水印修复 sidecar：原图 + 涂抹/框选笔画 → LaMa ONNX 修复 → PNG。

用法（由 Rust 侧拉起，路径均由参数传入）：
  python watermark_erase.py --src <原图> --strokes <笔画 json> --out <产物 png> --model <onnx>

依赖来源：
  PDFCONV_SITE_PACKAGES  cv2 / numpy（随包 Python 的外置依赖目录）
  WM_DEPS                 onnxruntime（单独叠加目录，不动 PDF 那套依赖）

stdout 只输出 JSON 行（进度与最终结果），由 Rust 逐行读取转成前端事件。
"""

import argparse
import json
import os
import sys
import time


# ---- 依赖目录叠加：必须在 import 之前完成 -----------------------------------
def _prepend(env_key):
    raw = os.environ.get(env_key)
    if not raw:
        return
    for part in raw.split(os.pathsep):
        part = part.strip()
        if part and os.path.isdir(part) and part not in sys.path:
            sys.path.insert(0, part)


_prepend("PDFCONV_SITE_PACKAGES")
_prepend("WM_DEPS")

import numpy as np  # noqa: E402
import cv2  # noqa: E402
import onnxruntime as ort  # noqa: E402


def emit(obj):
    """写一行 JSON，供 Rust 侧解析。ASCII 转义，避免控制台编码问题。"""
    sys.stdout.write(json.dumps(obj, ensure_ascii=True) + "\n")
    sys.stdout.flush()


def read_image(path):
    """cv2.imread 在 Windows 上读不了中文路径，统一走 fromfile + imdecode。"""
    try:
        buf = np.fromfile(path, dtype=np.uint8)
    except OSError:
        return None
    if buf.size == 0:
        return None
    return cv2.imdecode(buf, cv2.IMREAD_COLOR)


def write_image(path, img):
    """中文路径写出：imencode + tofile。"""
    ext = os.path.splitext(path)[1] or ".png"
    ok, buf = cv2.imencode(ext, img)
    if not ok:
        return False
    buf.tofile(path)
    return True


def _clamp(v, lo, hi):
    return max(lo, min(hi, v))


def build_mask(shape, strokes):
    """按笔画在与原图同尺寸的灰度 mask 上绘制待修复区域（白=待修复）。"""
    h, w = shape[:2]
    mask = np.zeros((h, w), np.uint8)
    for s in strokes:
        kind = s.get("kind")
        if kind == "brush":
            pts = s.get("points") or []
            radius = max(1, int(s.get("radius") or 6))
            xy = []
            for p in pts:
                if len(p) >= 2:
                    x = _clamp(int(p[0]), 0, w - 1)
                    y = _clamp(int(p[1]), 0, h - 1)
                    xy.append((x, y))
                    cv2.circle(mask, (x, y), radius, 255, -1)
            if len(xy) >= 2:
                arr = np.array(xy, dtype=np.int32)
                cv2.polylines(mask, [arr], False, 255, radius)
        elif kind == "rect":
            rc = s.get("rect") or []
            if len(rc) == 4:
                x1 = _clamp(int(rc[0]), 0, w - 1)
                y1 = _clamp(int(rc[1]), 0, h - 1)
                x2 = _clamp(int(rc[2]), 0, w - 1)
                y2 = _clamp(int(rc[3]), 0, h - 1)
                cv2.rectangle(mask, (x1, y1), (x2, y2), 255, -1)
    return mask


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--strokes", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--model", required=True)
    args = ap.parse_args()

    t0 = time.time()

    img = read_image(args.src)
    if img is None:
        emit({"t": "done", "status": "err", "message": "图片无法读取"})
        return 1
    h, w = img.shape[:2]

    try:
        with open(args.strokes, "r", encoding="utf-8") as f:
            strokes = json.load(f)
        if not isinstance(strokes, list):
            strokes = []
    except (OSError, ValueError):
        strokes = []

    mask = build_mask(img.shape, strokes)
    if mask.max() == 0:
        emit({"t": "done", "status": "err", "message": "未涂抹或框选任何区域"})
        return 1

    # 外扩一圈再羽化：避免选区边缘外的原图边框/笔画残留；推理用二值 mask，
    # 回贴用模糊后的灰度 alpha，避免修复边界生硬
    kernel = np.ones((5, 5), np.uint8)
    mask_d = cv2.dilate(mask, kernel, iterations=2)
    mask_blur = cv2.GaussianBlur(mask_d, (5, 5), 0)
    mask_bin = (mask_blur > 127).astype(np.uint8) * 255

    emit({"t": "p", "pct": 0.1, "label": "载入模型", "name": ""})
    try:
        sess = ort.InferenceSession(args.model, providers=["CPUExecutionProvider"])
        in_img = sess.get_inputs()[0].name
        in_mask = sess.get_inputs()[1].name
    except Exception as exc:  # noqa: BLE001
        emit({"t": "done", "status": "err", "message": "模型加载失败：{}".format(exc)})
        return 1

    emit({"t": "p", "pct": 0.35, "label": "预处理", "name": os.path.basename(args.src)})
    img_r = cv2.resize(img, (512, 512), interpolation=cv2.INTER_LINEAR)
    img_blob = (img_r.astype(np.float32) / 255.0)
    img_blob = np.ascontiguousarray(img_blob.transpose(2, 0, 1)[None])

    mask_r = cv2.resize(mask_bin, (512, 512), interpolation=cv2.INTER_NEAREST)
    mask_blob = (mask_r > 0).astype(np.float32)[None, None]

    emit({"t": "p", "pct": 0.55, "label": "修复中", "name": os.path.basename(args.src)})
    try:
        out = sess.run(None, {in_img: img_blob, in_mask: mask_blob})[0]
    except Exception as exc:  # noqa: BLE001
        emit({"t": "done", "status": "err", "message": "推理失败：{}".format(exc)})
        return 1

    res = np.transpose(out[0], (1, 2, 0))
    res8 = np.clip(res, 0, 255).astype(np.uint8)
    res8 = cv2.resize(res8, (w, h), interpolation=cv2.INTER_LINEAR)

    # 仅在 mask 区域用修复结果，区域外保持原图；羽化边缘按 alpha 混合
    alpha = (mask_blur.astype(np.float32) / 255.0)[:, :, None]
    merged = (img * (1.0 - alpha) + res8 * alpha).astype(np.uint8)

    emit({"t": "p", "pct": 0.9, "label": "写出结果", "name": ""})
    out_dir = os.path.dirname(args.out)
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)
    if not write_image(args.out, merged):
        emit({"t": "done", "status": "err", "message": "结果写出失败"})
        return 1

    emit(
        {
            "t": "done",
            "status": "ok",
            "out": args.out,
            "seconds": round(time.time() - t0, 1),
        }
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
