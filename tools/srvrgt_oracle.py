# -*- coding: utf-8 -*-
"""P0.3 独立 oracle：从原文直接解析 SRVRGT（键 IMPU，值 SN），验证"Text 列做键"这条路径。

不复用 ims_mem/ims_core/ims_mem2 的任何代码。

用法：
    python tools/srvrgt_oracle.py ..\\agcf_data\\ALLME_*.txt > temp\\srvrgt_py.txt
"""
import glob
import re
import sys

USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ADD_ME = re.compile(rb'^ADD ME:(.*);\s*$')
ADD_SRVRGT = re.compile(rb'^(?:ADD|MOD) SRVRGT:(.*);\s*$')


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


def text(v: bytes):
    return v.decode('gbk', 'replace')


def main(paths):
    files = []
    for p in paths:
        files.extend(sorted(glob.glob(p)) if any(ch in p for ch in '*?[') else [p])
    if not files:
        print('没有输入文件', file=sys.stderr)
        return 2

    rows = {}  # (ne, impu) -> sn
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
            m = ADD_SRVRGT.match(line)
            if not m or meid is None:
                continue
            ne = names.get(meid)
            if not ne:
                continue
            kv = dict(split_params(m.group(1)))
            impu = kv.get(b'IMPU')
            if not impu:
                continue
            rows[(ne, text(impu))] = text(kv[b'SN']) if b'SN' in kv else '-'

    lines = [f'{ne}|{impu}|{sn}' for (ne, impu), sn in rows.items()]
    for l in sorted(set(lines)):
        print(l)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
