# -*- coding: utf-8 -*-
"""P0.4 独立 oracle：从原文解析 SRTCOST（键 = CST,LP,PFX,PSR,SR；值 = SRN）。

要点：**PFX 的键语义按"半字节序列"比较**（`K'140` == `140`，但 `0140` != `140`），
与 Rust 侧 `PfxNum`（左对齐 BCD + 长度参与相等）同口径；不做数值归一化。

用法：
    python tools/srtcost_oracle.py ..\\agcf_data\\ALLME_*.txt > temp\\srtcost_py.txt
"""
import glob
import re
import sys

USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ADD_ME = re.compile(rb'^ADD ME:(.*);\s*$')
SRTCOST = re.compile(rb'^(?:ADD|MOD|SET) SRTCOST:(.*);\s*$')


def split_params(body: bytes):
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


def u(v, limit):
    if v is None or not v.isdigit():
        return None
    n = int(v)
    return n if n <= limit else None


def pfx_digits(v: bytes):
    """K'140 / 140 → (1,4,0)；非法返回 None。与 Rust 的 parse_pfx 同字符集。"""
    if v[:2] in (b"K'", b"k'"):
        v = v[2:]
    if not v or len(v) > 32:
        return None
    out = []
    for c in v:
        if 0x30 <= c <= 0x39:
            out.append(c - 0x30)
        elif 0x41 <= c <= 0x45:
            out.append(c - 0x41 + 10)
        elif 0x61 <= c <= 0x65:
            out.append(c - 0x61 + 10)
        elif c == 0x2A:  # '*'
            out.append(14)
        elif c == 0x23:  # '#'
            out.append(15)
        else:
            return None
    return tuple(out)


def text(v: bytes):
    return v.decode('gbk', 'replace')


def main(paths):
    files = []
    for p in paths:
        files.extend(sorted(glob.glob(p)) if any(ch in p for ch in '*?[') else [p])
    if not files:
        print('没有输入文件', file=sys.stderr)
        return 2

    rows = {}  # key -> (pfx_text, srn_text)
    for path in files:
        data = open(path, 'rb').read()
        names = {}
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r \t')
            m = ADD_ME.match(line)
            if not m:
                continue
            kv = dict(split_params(m.group(1)))
            if b'MEID' in kv and b'MENAME' in kv and kv[b'MEID'].isdigit():
                names[int(kv[b'MEID'])] = text(kv[b'MENAME'])

        meid = None
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r \t')
            m = USE_ME.match(line)
            if m:
                meid = int(m.group(1))
                continue
            m = SRTCOST.match(line)
            if not m or meid is None:
                continue
            ne = names.get(meid)
            if not ne:
                continue
            kv = dict(split_params(m.group(1)))
            cst = u(kv.get(b'CST'), 65535)
            lp = u(kv.get(b'LP'), 65535)
            psr = u(kv.get(b'PSR'), 255)
            sr = u(kv.get(b'SR'), 65535)
            pfx_raw = kv.get(b'PFX')
            pfd = pfx_digits(pfx_raw) if pfx_raw else None
            if None in (cst, lp, psr, sr) or pfd is None:
                continue
            key = (ne, cst, lp, pfd, psr, sr)
            rows[key] = (text(pfx_raw), text(kv[b'SRN']) if b'SRN' in kv else '-')

    lines = [
        f'{ne}|{cst}|{lp}|{pfx}|{psr}|{sr}|{srn}'
        for (ne, cst, lp, _pfd, psr, sr), (pfx, srn) in rows.items()
    ]
    for l in sorted(set(lines)):
        print(l)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
