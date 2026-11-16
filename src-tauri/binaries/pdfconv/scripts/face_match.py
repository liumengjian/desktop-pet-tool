#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""人脸比对 sidecar：两组照片 → 人脸特征 → 一一对应 → Word（每页一对）。

用法（由 Rust 侧拉起，路径均由参数传入）：
  python face_match.py --a <路径清单json> --b <路径清单json>
                       --out <docx> --det <onnx> --rec <onnx>
                       --group-a 第一组 --group-b 第二组 [--high 0.45] [--low 0.35]

依赖来源：
  PDFCONV_SITE_PACKAGES  cv2 / numpy / python-docx（随包 Python 的外置依赖目录）
  FACE_DEPS              onnxruntime（单独叠加目录，不动 PDF 那套依赖）

stdout 只输出 JSON 行（进度与最终结果），由 Rust 逐行读取转成前端事件。
"""

import argparse
import json
import os
import sys
import time
from io import BytesIO

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
_prepend("FACE_DEPS")

import numpy as np  # noqa: E402
import cv2  # noqa: E402


def emit(obj):
    """写一行 JSON，供 Rust 侧解析。ASCII 转义，避免控制台编码问题。"""
    sys.stdout.write(json.dumps(obj, ensure_ascii=True) + "\n")
    sys.stdout.flush()


# ---- 模型 ----------------------------------------------------------------
# ArcFace 标准 112x112 五点模板（左眼、右眼、鼻尖、左嘴角、右嘴角）
ARC_TEMPLATE = np.array(
    [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ],
    dtype=np.float32,
)

STRIDES = (8, 16, 32)
ANCHORS_PER_CELL = 2
DET_THRESH = 0.35
NMS_THRESH = 0.4
# 人脸框短边低于该像素（原图尺度）视为细节不足，转人工
MIN_FACE_PX = 24
# 短边低于该值视为低分辨率：裁脸放大重检，并在结果里标注「原图人脸偏小」
LOW_RES_FACE_PX = 112


def _session(path):
    import onnxruntime as ort

    opts = ort.SessionOptions()
    opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    # 几十人的批次用满 CPU 不划算，限制线程数避免抢界面
    opts.intra_op_num_threads = max(1, min(4, (os.cpu_count() or 2) - 1))
    opts.inter_op_num_threads = 1
    return ort.InferenceSession(path, opts, providers=["CPUExecutionProvider"])


class ScrfdDetector:
    """SCRFD（det_10g）：输出按元素数量归类，不依赖输出名顺序。"""

    def __init__(self, path):
        self.sess = _session(path)
        inp = self.sess.get_inputs()[0]
        self.input_name = inp.name
        self.out_names = [o.name for o in self.sess.get_outputs()]
        shape = list(inp.shape)
        self.static = all(isinstance(d, int) and d > 0 for d in shape[2:])
        self.static_wh = (int(shape[3]), int(shape[2])) if self.static else None

    def _det_size(self, want):
        if self.static:
            return self.static_wh
        # 动态形状：取 32 的倍数，控制上限
        w = max(320, min(1280, int(round(want[0] / 32.0)) * 32))
        h = max(320, min(1280, int(round(want[1] / 32.0)) * 32))
        return (w, h)

    def detect(self, bgr, upscale=1.0, det_want=640):
        """返回 [(score, box(4), kps(5x2))]，坐标为原图尺度。"""
        orig_h, orig_w = bgr.shape[:2]
        work = bgr
        if upscale != 1.0:
            work = cv2.resize(
                bgr,
                (max(1, int(round(orig_w * upscale))), max(1, int(round(orig_h * upscale)))),
                interpolation=cv2.INTER_LINEAR,
            )
        wh, ww = work.shape[:2]
        det_w, det_h = self._det_size((det_want, det_want))
        scale = min(det_w / float(ww), det_h / float(wh))
        nw, nh = max(1, int(round(ww * scale))), max(1, int(round(wh * scale)))
        resized = cv2.resize(work, (nw, nh), interpolation=cv2.INTER_LINEAR)
        canvas = np.zeros((det_h, det_w, 3), dtype=np.uint8)
        canvas[:nh, :nw] = resized

        blob = (canvas[:, :, ::-1].astype(np.float32) - 127.5) / 128.0
        blob = np.ascontiguousarray(blob.transpose(2, 0, 1)[None])

        outs = self.sess.run(self.out_names, {self.input_name: blob})
        decoded = self._decode(outs, det_h, det_w)
        if not decoded:
            return []

        boxes = np.stack([d[1] for d in decoded]).astype(np.float32)
        scores = np.array([d[0] for d in decoded], dtype=np.float32)
        kpss = np.stack([d[2] for d in decoded]).astype(np.float32)

        # 检测画布尺度 → 放大过的工作图 → 调用方原图
        boxes /= scale
        kpss /= scale
        if upscale != 1.0:
            boxes /= upscale
            kpss /= upscale

        keep = _nms(boxes, scores, NMS_THRESH)
        out = []
        for idx in keep:
            box = boxes[idx].copy()
            box[[0, 2]] = np.clip(box[[0, 2]], 0, orig_w)
            box[[1, 3]] = np.clip(box[[1, 3]], 0, orig_h)
            out.append((float(scores[idx]), box, kpss[idx]))
        return out

    def _decode(self, outs, det_h, det_w):
        """按「最后一维 = 1/4/10（score/bbox/kps）」+ 元素数反推 stride 归类。

        注意不能只按元素数归类：stride16 的 bbox 是 4×3200=12800，和 stride8 的
        score 12800 撞车，只按大小取第一个会拿错张量。
        """
        table = {}
        for arr in outs:
            a = np.asarray(arr)
            if a.ndim < 2:
                continue
            kind = a.shape[-1]
            if kind not in (1, 4, 10):
                continue
            numel = int(np.size(a))
            anchors = numel if kind == 1 else numel // kind
            if anchors <= 0:
                continue
            # N = (H/stride)*(W/stride)*每格2个anchor → stride = sqrt(2*H*W/N)
            stride_f = (2.0 * det_h * det_w / float(anchors)) ** 0.5
            stride = int(round(stride_f))
            if stride not in STRIDES:
                continue
            table[(stride, "s" if kind == 1 else ("b" if kind == 4 else "k"))] = a

        result = []
        for stride in STRIDES:
            s = table.get((stride, "s"))
            b = table.get((stride, "b"))
            k = table.get((stride, "k"))
            if s is None or b is None or k is None:
                continue
            h = det_h // stride
            w = det_w // stride
            centers = _anchor_centers(h, w, stride)
            scores = np.asarray(s, dtype=np.float32).reshape(-1)
            bbox = np.asarray(b, dtype=np.float32).reshape(-1, 4) * stride
            kps = np.asarray(k, dtype=np.float32).reshape(-1, 10) * stride
            if scores.shape[0] != centers.shape[0]:
                continue
            idx = np.where(scores >= DET_THRESH)[0]
            for i in idx:
                cx, cy = centers[i]
                box = np.array(
                    [
                        cx - bbox[i, 0],
                        cy - bbox[i, 1],
                        cx + bbox[i, 2],
                        cy + bbox[i, 3],
                    ],
                    dtype=np.float32,
                )
                pts = np.empty((5, 2), dtype=np.float32)
                for p in range(5):
                    pts[p, 0] = cx + kps[i, p * 2]
                    pts[p, 1] = cy + kps[i, p * 2 + 1]
                result.append((float(scores[i]), box, pts))
        return result


def _anchor_centers(h, w, stride):
    ys, xs = np.mgrid[0:h, 0:w]
    pts = np.stack([xs, ys], axis=-1).astype(np.float32).reshape(-1, 2) * stride
    if ANCHORS_PER_CELL > 1:
        pts = np.repeat(pts, ANCHORS_PER_CELL, axis=0)
    return pts


def _nms(boxes, scores, thresh):
    if boxes.size == 0:
        return []
    x1, y1, x2, y2 = boxes[:, 0], boxes[:, 1], boxes[:, 2], boxes[:, 3]
    areas = np.maximum(0.0, x2 - x1) * np.maximum(0.0, y2 - y1)
    order = scores.argsort()[::-1]
    keep = []
    while order.size > 0:
        i = int(order[0])
        keep.append(i)
        xx1 = np.maximum(x1[i], x1[order[1:]])
        yy1 = np.maximum(y1[i], y1[order[1:]])
        xx2 = np.minimum(x2[i], x2[order[1:]])
        yy2 = np.minimum(y2[i], y2[order[1:]])
        w = np.maximum(0.0, xx2 - xx1)
        h = np.maximum(0.0, yy2 - yy1)
        inter = w * h
        union = areas[i] + areas[order[1:]] - inter
        iou = np.where(union > 0, inter / union, 0.0)
        order = order[1:][iou <= thresh]
    return keep


class ArcFace:
    def __init__(self, path):
        self.sess = _session(path)
        inp = self.sess.get_inputs()[0]
        self.input_name = inp.name
        self.out_names = [o.name for o in self.sess.get_outputs()]

    def embed(self, aligned_bgr):
        blob = (aligned_bgr[:, :, ::-1].astype(np.float32) - 127.5) / 127.5
        blob = np.ascontiguousarray(blob.transpose(2, 0, 1)[None])
        out = self.sess.run(self.out_names, {self.input_name: blob})[0]
        vec = np.asarray(out, dtype=np.float32).reshape(-1)
        norm = float(np.linalg.norm(vec))
        if norm <= 0:
            return None
        return vec / norm


# ---- 读图与特征 -----------------------------------------------------------
def read_image(path):
    """cv2.imread 在 Windows 上读不了中文路径，统一走 fromfile + imdecode。"""
    try:
        buf = np.fromfile(path, dtype=np.uint8)
    except OSError:
        return None
    if buf.size == 0:
        return None
    img = cv2.imdecode(buf, cv2.IMREAD_COLOR)
    if img is None:
        return None
    # EXIF 方向由 OpenCV 解码时自动处理（imdecode 默认不忽略该标记），无需再转
    return img


def enhance_if_dark(bgr):
    """暗光 / 逆光自拍先做 CLAHE，只用于检测，不改变几何。"""
    gray = cv2.cvtColor(bgr, cv2.COLOR_BGR2GRAY)
    if float(gray.mean()) >= 70.0:
        return bgr, False
    lab = cv2.cvtColor(bgr, cv2.COLOR_BGR2LAB)
    l, a, b = cv2.split(lab)
    l = cv2.createCLAHE(clipLimit=2.0, tileGridSize=(8, 8)).apply(l)
    return cv2.cvtColor(cv2.merge([l, a, b]), cv2.COLOR_LAB2BGR), True


def _refine_face(img, det, box):
    """小脸补救：裁出人脸邻域放大后重新检测关键点。

    返回 (work_img, box, kps)，坐标都在 work_img 尺度；失败返回 None。
    整图放大没有意义——detect() 会把图重新缩放塞回 640 检测视口，
    有效分辨率不变；只有裁脸再放大才能真正提高关键点精度。
    """
    h, w = img.shape[:2]
    side = float(min(box[2] - box[0], box[3] - box[1]))
    if side < 4.0:
        return None
    cx, cy = (box[0] + box[2]) / 2.0, (box[1] + box[3]) / 2.0
    half = side * 2.5
    x0, y0 = max(0, int(round(cx - half))), max(0, int(round(cy - half)))
    x1, y1 = min(w, int(round(cx + half))), min(h, int(round(cy + half)))
    if x1 - x0 < 12 or y1 - y0 < 12:
        return None
    crop = img[y0:y1, x0:x1]
    zoom = 640.0 / float(max(crop.shape[:2]))
    if zoom < 1.2:
        return None
    work = cv2.resize(crop, None, fx=zoom, fy=zoom, interpolation=cv2.INTER_CUBIC)
    faces = det.detect(work, upscale=1.0, det_want=640)
    if not faces:
        return None
    wcx, wcy = work.shape[1] / 2.0, work.shape[0] / 2.0
    best = min(
        faces,
        key=lambda f: ((f[1][0] + f[1][2]) / 2.0 - wcx) ** 2
        + ((f[1][1] + f[1][3]) / 2.0 - wcy) ** 2,
    )
    return work, best[1], best[2]


def analyze(path, det, rec):
    """返回 dict：ok / reason / emb / face_px / lowq。"""
    img = read_image(path)
    if img is None:
        return {"ok": False, "reason": "unreadable"}
    if min(img.shape[0], img.shape[1]) < 80:
        return {"ok": False, "reason": "too_small"}

    det_img, enhanced = enhance_if_dark(img)
    faces = det.detect(det_img, upscale=1.0, det_want=640)
    if not faces:
        return {"ok": False, "reason": "no_face", "enhanced": enhanced}

    faces.sort(key=lambda f: (f[1][2] - f[1][0]) * (f[1][3] - f[1][1]), reverse=True)
    score, box, kps = faces[0]
    short_side = float(min(box[2] - box[0], box[3] - box[1]))

    # 多人合影：第二张脸面积占比过大时，不自动判主体
    if len(faces) > 1:
        area0 = (box[2] - box[0]) * (box[3] - box[1])
        area1 = (faces[1][1][2] - faces[1][1][0]) * (faces[1][1][3] - faces[1][1][1])
        if area1 > area0 * 0.6:
            return {"ok": False, "reason": "multi_face", "face_count": len(faces)}

    if short_side < MIN_FACE_PX:
        return {"ok": False, "reason": "face_too_small", "face_px": int(short_side)}

    # 小脸：先裁脸放大重检，关键点落在放大图上，对齐更准
    lowq = short_side < LOW_RES_FACE_PX
    work, wkps = img, kps
    refined = False
    if lowq:
        got = _refine_face(img, det, box)
        if got is not None:
            work, _, wkps = got
            refined = True

    m, _ = cv2.estimateAffinePartial2D(
        wkps.astype(np.float32), ARC_TEMPLATE, method=cv2.LMEDS
    )
    if m is None:
        return {"ok": False, "reason": "align_failed"}
    aligned = cv2.warpAffine(work, m, (112, 112), borderValue=0.0)
    emb = rec.embed(aligned)
    if emb is None:
        return {"ok": False, "reason": "embed_failed"}
    return {
        "ok": True,
        "emb": emb,
        "score": round(score, 3),
        "face_px": int(short_side),
        "lowq": lowq,
        "refined": refined,
        "enhanced": enhanced,
        "face_count": len(faces),
    }


# ---- 一一对应 -------------------------------------------------------------
def assign(scores, low):
    """全局最优一一对应（匈牙利 + 哑元处理数量不等）。返回 (i, j) 列表。"""
    n, m = scores.shape
    if n == 0 or m == 0:
        return []
    size = n + m
    cost = np.zeros((size, size), dtype=np.float64)
    for i in range(n):
        for j in range(m):
            s = float(scores[i, j])
            cost[i, j] = 0.0 if s < low else -s
    idx = _hungarian(cost)
    pairs = []
    for i, j in idx:
        if i < n and j < m and scores[i, j] >= low:
            pairs.append((i, j))
    return pairs


def _hungarian(cost):
    """经典 O(n^3) 匈牙利算法（最小代价指派），返回 [(行, 列)]。"""
    n = cost.shape[0]
    m = cost.shape[1]
    assert n <= m
    u = np.zeros(n + 1)
    v = np.zeros(m + 1)
    p = np.zeros(m + 1, dtype=int)
    way = np.zeros(m + 1, dtype=int)
    inf = float("inf")
    for i in range(1, n + 1):
        p[0] = i
        j0 = 0
        minv = np.full(m + 1, inf)
        used = np.zeros(m + 1, dtype=bool)
        while True:
            used[j0] = True
            i0 = p[j0]
            delta = inf
            j1 = 0
            row = cost[i0 - 1]
            for j in range(1, m + 1):
                if used[j]:
                    continue
                cur = row[j - 1] - u[i0] - v[j]
                if cur < minv[j]:
                    minv[j] = cur
                    way[j] = j0
                if minv[j] < delta:
                    delta = minv[j]
                    j1 = j
            for j in range(0, m + 1):
                if used[j]:
                    u[p[j]] += delta
                    v[j] -= delta
                else:
                    minv[j] -= delta
            j0 = j1
            if p[j0] == 0:
                break
        while True:
            j1 = way[j0]
            p[j0] = p[j1]
            j0 = j1
            if j0 == 0:
                break
    out = []
    for j in range(1, m + 1):
        if p[j] != 0:
            out.append((int(p[j]) - 1, j - 1))
    return out


# ---- Word ----------------------------------------------------------------
def jpeg_stream(path, max_edge=1200, quality=82):
    img = read_image(path)
    if img is None:
        return None
    h, w = img.shape[:2]
    s = min(1.0, float(max_edge) / max(h, w))
    if s < 1.0:
        img = cv2.resize(
            img, (max(1, int(round(w * s))), max(1, int(round(h * s)))),
            interpolation=cv2.INTER_AREA,
        )
    ok, buf = cv2.imencode(".jpg", img, [int(cv2.IMWRITE_JPEG_QUALITY), quality])
    if not ok:
        return None
    return BytesIO(buf.tobytes()), int(img.shape[1]), int(img.shape[0])


def write_docx(out_path, pairs, unmatched_a, unmatched_b, skipped, args):
    from docx import Document
    from docx.enum.text import WD_ALIGN_PARAGRAPH
    from docx.oxml.ns import qn
    from docx.shared import Cm, Pt

    doc = Document()
    sec = doc.sections[0]
    sec.page_width = Cm(21.0)
    sec.page_height = Cm(29.7)
    sec.left_margin = Cm(2.0)
    sec.right_margin = Cm(2.0)
    sec.top_margin = Cm(1.8)
    sec.bottom_margin = Cm(1.8)

    style = doc.styles["Normal"]
    style.font.name = "Microsoft YaHei"
    style.font.size = Pt(10)
    style.element.rPr.rFonts.set(qn("w:eastAsia"), "Microsoft YaHei")

    # 一页只放一对照片：同段两图并排，缩进与段落间距全部清零，居中
    def put_pair(paths):
        p = doc.add_paragraph()
        p.alignment = WD_ALIGN_PARAGRAPH.CENTER
        p.paragraph_format.space_before = Pt(0)
        p.paragraph_format.space_after = Pt(0)
        p.paragraph_format.line_spacing = 1.0
        for k, path in enumerate(paths):
            got = jpeg_stream(path)
            if got is None:
                p.add_run("（图片无法读取）")
            else:
                stream, iw, ih = got
                # 竖图按高度封顶，横图按宽度封顶，两张并排不超版心
                if ih / float(iw) > 10.3 / 7.5:
                    p.add_run().add_picture(stream, height=Cm(10.3))
                else:
                    p.add_run().add_picture(stream, width=Cm(7.5))
            if k == 0:
                p.add_run("   ")

    total = len(pairs)
    for n, item in enumerate(pairs, start=1):
        put_pair((item["a"], item["b"]))
        if n < total:
            doc.add_page_break()

    # ---- 最后一页：只写没配上的部分 ----
    if unmatched_a or unmatched_b or skipped:
        if pairs:
            doc.add_page_break()

        def block(text, bold=False, size=9):
            p = doc.add_paragraph()
            run = p.add_run(text)
            run.bold = bold
            run.font.size = Pt(size)
            run.font.name = "Microsoft YaHei"
            run._element.rPr.rFonts.set(qn("w:eastAsia"), "Microsoft YaHei")

        block(
            "共配对 {} 组。{} 未匹配 {} 张，{} 未匹配 {} 张，无法自动处理 {} 张。".format(
                total,
                args.group_a,
                len(unmatched_a),
                args.group_b,
                len(unmatched_b),
                len(skipped),
            ),
            bold=True,
            size=11,
        )
        if unmatched_a:
            block("{}：".format(args.group_a), bold=True)
            for it in unmatched_a:
                block("· {}    最高分 {:.2f}".format(os.path.basename(it["path"]), it["best"]))
        if unmatched_b:
            block("{}：".format(args.group_b), bold=True)
            for it in unmatched_b:
                block("· {}    最高分 {:.2f}".format(os.path.basename(it["path"]), it["best"]))
        if skipped:
            block("无法自动处理：", bold=True)
            for it in skipped:
                block("· {}    {}".format(os.path.basename(it["path"]), it["reason"]))

    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
    doc.save(out_path)


# ---- 主流程 ---------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", required=True, help="A 组路径清单（JSON 数组文件或行分隔）")
    ap.add_argument("--b", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--det", required=True)
    ap.add_argument("--rec", required=True)
    ap.add_argument("--group-a", default="第一组")
    ap.add_argument("--group-b", default="第二组")
    ap.add_argument("--high", type=float, default=0.4)
    ap.add_argument("--low", type=float, default=0.25)
    args = ap.parse_args()

    def load_list(raw):
        if not raw:
            return []
        with open(raw, "r", encoding="utf-8") as f:
            data = f.read()
        try:
            parsed = json.loads(data)
            if isinstance(parsed, list):
                return [str(x) for x in parsed]
        except ValueError:
            pass
        return [ln.strip() for ln in data.splitlines() if ln.strip()]

    group_a = load_list(args.a)
    group_b = load_list(args.b)
    if not group_a or not group_b:
        emit({"t": "done", "status": "err", "message": "两组都需要至少一张照片"})
        return 1

    total = len(group_a) + len(group_b)
    done = 0
    t0 = time.time()

    emit({"t": "p", "pct": 0, "label": "载入模型", "name": ""})
    try:
        det = ScrfdDetector(args.det)
        rec = ArcFace(args.rec)
    except Exception as exc:  # noqa: BLE001
        emit({"t": "done", "status": "err", "message": "模型加载失败：{}".format(exc)})
        return 1

    def run_group(paths, label):
        nonlocal done
        embs, good, bad = [], [], []
        for path in paths:
            emit({"t": "p", "pct": 5 + 90.0 * done / total, "label": label, "name": os.path.basename(path)})
            info = analyze(path, det, rec)
            if info.get("ok"):
                embs.append(info["emb"])
                good.append(
                    {
                        "path": path,
                        "lowq": bool(info.get("lowq")),
                        "face_px": int(info.get("face_px") or 0),
                    }
                )
            else:
                bad.append({"path": path, "reason": _reason_text(info.get("reason"))})
            done += 1
            emit({"t": "p", "pct": 5 + 90.0 * done / total, "label": label, "name": os.path.basename(path)})
        return embs, good, bad

    emb_a, ok_a, bad_a = run_group(group_a, args.group_a)
    emb_b, ok_b, bad_b = run_group(group_b, args.group_b)

    skipped = bad_a + bad_b
    pairs, unmatched_a, unmatched_b = [], [], []

    if emb_a and emb_b:
        mat_a = np.stack(emb_a)
        mat_b = np.stack(emb_b)
        scores = mat_a @ mat_b.T
        assigned = assign(scores, args.low)
        used_a, used_b = set(), set()
        for i, j in assigned:
            used_a.add(i)
            used_b.add(j)
            s = float(scores[i, j])
            a_item, b_item = ok_a[i], ok_b[j]
            lowq = bool(a_item["lowq"] or b_item["lowq"])
            pairs.append(
                {
                    "a": a_item["path"],
                    "b": b_item["path"],
                    "score": s,
                    "lowq": lowq,
                    "status": "自动通过" if s >= args.high else "待人工复核",
                }
            )
        pairs.sort(key=lambda x: -x["score"])
        for i, item in enumerate(ok_a):
            if i not in used_a:
                unmatched_a.append({"path": item["path"], "best": float(scores[i].max())})
        for j, item in enumerate(ok_b):
            if j not in used_b:
                unmatched_b.append({"path": item["path"], "best": float(scores[:, j].max())})

    emit({"t": "p", "pct": 96, "label": "生成 Word", "name": ""})
    try:
        write_docx(args.out, pairs, unmatched_a, unmatched_b, skipped, args)
    except Exception as exc:  # noqa: BLE001
        emit({"t": "done", "status": "err", "message": "生成文档失败：{}".format(exc)})
        return 1

    emit(
        {
            "t": "done",
            "status": "ok",
            "out": args.out,
            "pairs": len(pairs),
            "gray": sum(1 for p in pairs if p["status"] != "自动通过"),
            "unmatchedA": len(unmatched_a),
            "unmatchedB": len(unmatched_b),
            "skipped": len(skipped),
            "seconds": round(time.time() - t0, 1),
        }
    )
    return 0


def _reason_text(code):
    return {
        "unreadable": "图片无法读取",
        "too_small": "分辨率过低",
        "no_face": "未检测到人脸",
        "multi_face": "多张脸，无法判定主体",
        "face_too_small": "人脸过小",
        "align_failed": "对齐失败",
        "embed_failed": "特征提取失败",
    }.get(code, code or "未知原因")


if __name__ == "__main__":
    sys.exit(main())
