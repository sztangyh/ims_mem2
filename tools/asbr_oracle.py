# -*- coding: utf-8 -*-
"""ASBR 值级独立 oracle：从原文解析，输出与 `examples/asbr_hash.rs` 相同的
`NE|行数|FNV1a64(排序后的规范行)`。

规范串（19 列，缺参写 `-`，行按整串升序）：
    pui|pri|did|eid|tid|v5iid|l3addr|isdn|trunkgp|mn|netid|phncon|netinfo|digmap|ifmimn|cxg|pwd|sgn|globdmapidx|

用法：
    python tools/asbr_oracle.py ..\\agcf_data\\ALLME_*.txt
"""
import glob
import re
import sys

USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ADD_ME = re.compile(rb'^ADD ME:(.*);\s*$')
ADD_ASBR = re.compile(rb'^(?:ADD|MOD) ASBR:(.*);\s*$')

TEXT_FIELDS = ['PUI', 'PRI', 'EID', 'NETID', 'PHNCON', 'PWD']
LABEL_FIELDS = ['DID', 'NETINFO', 'SGN']
NUM_FIELDS = [
    ('TID', 0xFFFFFFFF),
    ('V5IID', 0xFFFFFFFF),
    ('L3ADDR', 0xFFFFFFFF),
    ('ISDN', 255),
    ('TRUNKGP', 0xFFFFFFFF),
    ('MN', 65535),
    ('DIGMAP', 65535),
    ('IFMIMN', 65535),
    ('CXG', 65535),
    ('GLOBDMAPIDX', 0xFFFFFFFF),
]
ORDER = ['PUI', 'PRI', 'DID', 'EID', 'TID', 'V5IID', 'L3ADDR', 'ISDN', 'TRUNKGP',
         'MN', 'NETID', 'PHNCON', 'NETINFO', 'DIGMAP', 'IFMIMN', 'CXG', 'PWD',
         'SGN', 'GLOBDMAPIDX']


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


def fnv1a64(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for c in data:
        h ^= c
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def text(v: bytes) -> str:
    return v.decode('gbk', 'replace')


def main(paths):
    files = []
    for p in paths:
        files.extend(sorted(glob.glob(p)) if any(ch in p for ch in '*?[') else [p])
    if not files:
        print('没有输入文件', file=sys.stderr)
        return 2

    per_ne = {}
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
            m = ADD_ASBR.match(line)
            if not m or meid is None:
                continue
            ne = names.get(meid)
            if not ne:
                continue
            kv = dict(split_params(m.group(1)))
            pui = kv.get(b'PUI')
            if not pui:
                continue
            per_ne.setdefault(ne, {})[text(pui)] = kv  # ADD 后写覆盖

    for ne in sorted(per_ne):
        rows = []
        for pui, kv in per_ne[ne].items():
            parts = []
            for f in ORDER:
                v = kv.get(f.encode())
                if f in TEXT_FIELDS or f in LABEL_FIELDS:
                    parts.append(text(v) if v is not None else '-')
                else:
                    limit = dict(NUM_FIELDS)[f]
                    if v is not None and v.isdigit() and int(v) <= limit:
                        parts.append(str(int(v)))
                    else:
                        parts.append('-')
            rows.append('|'.join(parts) + '|')
        rows.sort()
        import os
        if os.environ.get('ASBR_DEBUG'):
            print(f'{ne}: {rows[0]}', file=sys.stderr)
        joined = '\n'.join(rows).encode('utf-8')
        print(f'{ne}|{len(rows)}|{fnv1a64(joined):016x}')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
