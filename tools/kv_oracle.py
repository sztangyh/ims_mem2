# -*- coding: utf-8 -*-
"""独立 oracle：`CNACLD / CLRDSN / PRATG / PRALNK / PRATKC` 的规范形（与 `kv_dump.rs` 同规范）。

规范：`NE|(按声明序的字段…)`，缺参 `-`，枚举写词，布尔 `1/0`，字冠/位串写原文。
去重口径：按自然键 last-wins（ADD 后写覆盖）。

用法：
    python tools/kv_oracle.py cnacld ..\\agcf_data\\ALLME_*.txt
"""
import glob
import re
import sys

USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ADD_ME = re.compile(rb'^ADD ME:(.*);\s*$')

# moc: (MOC 名, [(字段, 种类, 上限)], 键字段)
# 种类：t=文本 n=数字 b=布尔 e=枚举
SPEC = {
    'cnacld': ('CNACLD', [
        ('LDN', 't', 0), ('LP', 'n', 65535), ('PFX', 't', 0), ('CSA', 'e', 0),
        ('RSC', 'n', 0xFFFFFFFF), ('MINL', 'n', 255), ('MAXL', 'n', 255),
        ('CLRAF', 'b', 0), ('CSTP', 'e', 0), ('IAC', 'b', 0), ('GAIN', 'e', 0),
        ('RDT', 'n', 65535), ('ECOS', 'b', 0), ('SPCHG', 'e', 0),
        ('SDESCRIPTION', 't', 0), ('OTDISPID', 'n', 0xFFFFFFFF),
    ], ['LP', 'PFX']),
    'clrdsn': ('CLRDSN', [
        ('DSP', 'n', 65535), ('CLI', 't', 0), ('DSTYPE', 'e', 0), ('FUNC', 'e', 0),
        ('DAI', 'e', 0), ('MGWF', 'e', 0), ('MIN', 'n', 255), ('MAX', 'n', 255),
        ('RDCX', 'n', 65535), ('DCF', 'b', 0), ('CLIRFLAG', 'b', 0),
    ], ['DSP', 'CLI']),
    'pratg': ('PRATG', [
        ('TG', 'n', 0xFFFFFFFF), ('TGN', 't', 0), ('MGW', 't', 0), ('SRC', 'n', 0xFFFFFFFF),
        ('LINK', 'n', 0xFFFFFFFF), ('CDEF', 't', 0), ('CSC', 'n', 65535), ('RCHS', 'n', 0xFFFFFFFF),
        ('EHC', 'b', 0), ('ICR', 't', 0), ('OCR', 't', 0), ('EVA', 't', 0),
        ('SVRCTRL', 't', 0), ('SVRCTRLS', 't', 0), ('SVRCTRLT', 't', 0),
        ('LISTOFCODEC', 't', 0), ('IOI', 't', 0), ('ISBF', 'b', 0), ('ISDP', 'b', 0),
        ('NICF', 'b', 0), ('LMTGRP', 'n', 65535), ('MAXI', 'n', 0xFFFFFFFF),
        ('MTG', 'n', 0xFFFFFFFF), ('TCLITYP', 'e', 0), ('TGMS', 'e', 0),
    ], ['TG']),
    'pralnk': ('PRALNK', [
        ('PLN', 'n', 0xFFFFFFFF), ('LNKNAME', 't', 0), ('SCN', 'n', 0xFFFFFFFF),
        ('MN', 'n', 65535), ('LKS', 'n', 65535), ('BINIFID', 'n', 0xFFFFFFFF),
        ('SIGT', 'e', 0), ('OGCRL', 'n', 0xFFFFFFFF), ('NCF', 'e', 0),
        ('RCX', 'n', 0xFFFFFFFF), ('CCI', 'n', 0xFFFFFFFF), ('NV', 'e', 0),
    ], ['PLN']),
    'rt': ('RT', [
        ('R', 'n', 65535),
    ] + [(f'PERC{i}', 'n', 255) for i in range(1, 29)]
      + [(f'PERC{i}', 't', 0) for i in range(29, 33)]
      + [('PERCCFG', 'b', 0), ('RN', 't', 0)]
      + [(f'SR{i}', 'n', 65535) for i in range(1, 29)]
      + [('SRST', 'e', 0)], ['R']),
    'pratkc': ('PRATKC', [
        ('MN', 'n', 65535), ('TG', 'n', 0xFFFFFFFF), ('TGN', 't', 0),
        ('SC', 'n', 0xFFFFFFFF), ('EC', 'n', 0xFFFFFFFF), ('TID', 'n', 0xFFFFFFFF),
        ('CS', 'e', 0), ('TIDPFX', 'e', 0),
    ], ['MN', 'TG', 'SC']),
    # D53 步骤 2：N7TKC 在内存里是稀疏 Circuit + N7TG 句柄，dump 展开回这 9 列
    'n7tkc': ('N7TKC', [
        ('MN', 'n', 65535), ('SC', 'n', 65535), ('CS', 't', 0), ('EC', 'n', 65535),
        ('SCF', 'b', 0), ('SCIC', 'n', 65535), ('TG', 'n', 65535), ('TGN', 't', 0),
        ('TID', 'n', 65535),
    ], ['MN', 'SC']),
}


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


def text(v: bytes) -> str:
    return v.decode('gbk', 'replace')


def val(v, kind, limit):
    if v is None:
        return '-'
    if kind in ('t', 'e'):
        return text(v)
    if kind == 'b':
        u = v.upper()
        if u in (b'YES', b'TRUE', b'1'):
            return '1'
        if u in (b'NO', b'FALSE', b'0'):
            return '0'
        return '-'
    if v.isdigit() and int(v) <= limit:
        return str(int(v))
    return '-'


def main(argv):
    if len(argv) < 2:
        print('用法: kv_oracle.py <cnacld|clrdsn|pratg|pralnk|pratkc> <文件...>', file=sys.stderr)
        return 2
    key = argv[0].lower()
    if key not in SPEC:
        print(f'不认识的 MOC: {key}', file=sys.stderr)
        return 2
    moc, fields, keys = SPEC[key]
    pat = re.compile(rb'^(?:ADD|MOD|SET) ' + moc.encode() + rb':(.*);\s*$')
    key_pos = [i for i, (f, _, _) in enumerate(fields) if f in keys]

    files = []
    for p in argv[1:]:
        files.extend(sorted(glob.glob(p)) if any(ch in p for ch in '*?[') else [p])
    if not files:
        print('没有输入文件', file=sys.stderr)
        return 2

    dedup = {}
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
            m = pat.match(line)
            if not m or meid is None:
                continue
            ne = names.get(meid)
            if not ne:
                continue
            kv = dict(split_params(m.group(1)))
            parts = [val(kv.get(f.encode()), k, lim) for (f, k, lim) in fields]
            # 键缺任何一个 → 该行不成立（与 Table 的 key_missing 同口径）
            if any(parts[i] == '-' for i in key_pos):
                continue
            dedup[(ne, tuple(parts[i] for i in key_pos))] = '|'.join([ne] + parts)

    for r in sorted(dedup.values()):
        print(r)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
