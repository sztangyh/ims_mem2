# -*- coding: utf-8 -*-
"""P0.2 独立 oracle：从**原文**直接解析 DNC，产出与 `examples/dnc_dump.rs` 相同的规范形。

不复用 ims_mem/ims_core 的任何代码 —— 这是"第二实现"，用来对拍 ims_mem2 的导入结果。

用法：
    python tools/dnc_oracle.py ..\\agcf_data\\ALLME_*.txt > temp\\dnc_py.txt
"""
import glob
import re
import sys

USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ADD_ME = re.compile(rb'^ADD ME:(.*);\s*$')
ADD_DNC = re.compile(rb'^ADD DNC:(.*);\s*$')


def split_params(body: bytes):
    """按逗号切参数（引号内逗号不算），返回 [(大写名, 去引号值)]。"""
    out, i, start, inq = [], 0, 0, False
    while i < len(body):
        c = body[i]
        if c == 0x22:
            inq = not inq
        elif c == 0x2C and not inq:
            out.append(body[start:i])
            start = i + 1
        i += 1
    out.append(body[start:])
    kv = []
    for seg in out:
        if b'=' not in seg:
            continue
        k, v = seg.split(b'=', 1)
        k = k.strip().upper()
        v = v.strip()
        if len(v) >= 2 and v[0] == 0x22 and v[-1] == 0x22:
            v = v[1:-1]
        if k:
            kv.append((k, v))
    return kv


def u8(v: bytes):
    """与 Rust 侧 `parse_u8` 同口径：纯数字且 <=255，否则视为缺参。"""
    if not v or len(v) > 3 or not v.isdigit():
        return None
    n = int(v)
    return n if n <= 255 else None


def text(v: bytes):
    return v.decode('gbk', 'replace')


def main(paths):
    files = []
    for p in paths:
        files.extend(sorted(glob.glob(p)) if any(ch in p for ch in '*?[') else [p])
    if not files:
        print('没有输入文件', file=sys.stderr)
        return 2

    rows = {}  # (ne_name, dcx) -> dict
    for path in files:
        data = open(path, 'rb').read()
        # 第一遍：本文件的 MEID -> MENAME（CGP 段的 ADD ME 清单）
        names = {}
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r \t')
            m = ADD_ME.match(line)
            if not m:
                continue
            kv = dict(split_params(m.group(1)))
            if b'MEID' in kv and b'MENAME' in kv and kv[b'MEID'].isdigit():
                names[int(kv[b'MEID'])] = text(kv[b'MENAME'])

        # 第二遍：DNC 行（ADD 后写覆盖，与 Table 的 dup_add 同口径）
        meid = None
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r \t')
            m = USE_ME.match(line)
            if m:
                meid = int(m.group(1))
                continue
            m = ADD_DNC.match(line)
            if not m or meid is None:
                continue
            ne = names.get(meid)
            if not ne:
                continue
            kv = dict(split_params(m.group(1)))
            dcx = kv.get(b'DCX')
            if not dcx or not dcx.isdigit() or int(dcx) > 65535:
                continue
            rows[(ne, int(dcx))] = {
                'DCT': text(kv[b'DCT']) if b'DCT' in kv else '-',
                'ND': text(kv[b'ND']) if b'ND' in kv else '-',
                'ACN': text(kv[b'ACN']) if b'ACN' in kv else '-',
                'DAI': text(kv[b'DAI']) if b'DAI' in kv else '-',
                'DCL': u8(kv.get(b'DCL', b'')),
                'DCP': u8(kv.get(b'DCP', b'')),
            }

    lines = []
    for (ne, dcx), r in rows.items():
        dcl = '-' if r['DCL'] is None else str(r['DCL'])
        dcp = '-' if r['DCP'] is None else str(r['DCP'])
        lines.append(f"{ne}|{dcx}|{r['DCT']}|{r['ND']}|{r['ACN']}|{r['DAI']}|{dcl}|{dcp}")
    for l in sorted(set(lines)):
        print(l)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))

