# -*- coding: utf-8 -*-
"""引用名一致性护栏（A6，源级）：名称列 ≡ f(数值键)？

背景（移植自 `ims_mem/codegen/check_refs.py`，2026-10-20 决策）：行内同时携带数值键 + 名称键
（`SR/SRN`、`LP/LDN`、`TG/TGN`）时，名称是**冗余副本**。存不存是一回事，**一致不一致**必须有人盯着：
任何"名称漂移"（引用值 ≠ 定义表值）都报警。

为什么 ims_mem2 需要两份：
- **本脚本（源级）**：只读原文，覆盖**未声明参数**（提升列里的名称，如 `CNACLR.LDN`）；
- `cargo run --example refs_name_check`（存储级）：只读内存表，覆盖**我们实际存的列**，
  和 `refs` 的命中统计是两件事（那边只数命中/悬空，不看名字对不对）。

用法：
    python tools/check_refs.py [文件...]      # 默认 ../agcf_data/ALLME_*.txt
退出码 0 = 全部一致。
"""
import collections
import glob
import re
import sys

DEFAULT = r'..\agcf_data\ALLME_*.txt'
USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ADD_ME = re.compile(rb'^ADD ME:(.*);\s*$')
MML = re.compile(rb'^(ADD|MOD|SET)\s+([A-Z0-9]+):')

# 校验规则：(引用方对象, 数值键参数, 名称参数, 定义对象列表, 定义键参数, 定义名参数)
# 定义对象允许多来源（TGN 的定义源 = PRATG/SIPTG/N7TG 并集，与 ims_core trunk.tgn_map 同口径）
RULES = [
    ('SRTCOST', 'SR', 'SRN', ('SRT',), 'SRC', 'SRN'),
    ('SRTATTR', 'R', 'RN', ('RT',), 'R', 'RN'),
    ('SRTATTR', 'SR', 'SRN', ('SRT',), 'SRC', 'SRN'),
    ('TGSRT', 'SRC', 'SRN', ('SRT',), 'SRC', 'SRN'),
    ('CALLSRC', 'LP', 'LDN', ('LDNSET',), 'LP', 'LDN'),
    ('PFXPRO', 'LP', 'LDN', ('LDNSET',), 'LP', 'LDN'),
    ('CNACLD', 'LP', 'LDN', ('LDNSET',), 'LP', 'LDN'),
    ('CNACLR', 'LP', 'LDN', ('LDNSET',), 'LP', 'LDN'),
    ('TGDSG', 'TG', 'TGN', ('PRATG', 'SIPTG', 'N7TG'), 'TG', 'TGN'),
    ('PRATKC', 'TG', 'TGN', ('PRATG', 'SIPTG', 'N7TG'), 'TG', 'TGN'),
]


def split_params(body):
    out = []
    i = 0
    inq = False
    start = 0
    while i < len(body):
        c = body[i]
        if c == 0x22:
            inq = not inq
        elif c == 0x2C and not inq:
            seg = body[start:i]
            eq = seg.find(b'=')
            if eq > 0:
                out.append((seg[:eq].strip().upper().decode(), seg[eq + 1:].strip()))
            start = i + 1
        i += 1
    seg = body[start:]
    eq = seg.find(b'=')
    if eq > 0:
        out.append((seg[:eq].strip().upper().decode(), seg[eq + 1:].strip()))
    return out


def unq(v):
    return v[1:-1] if len(v) >= 2 and v[0] == 0x22 and v[-1] == 0x22 else v


def gbk(v):
    try:
        return v.decode('gbk')
    except Exception:  # noqa: BLE001
        return v.hex()


def main(argv):
    paths = []
    for p in (argv or [DEFAULT]):
        paths.extend(sorted(glob.glob(p)) if any(c in p for c in '*?[') else [p])
    if not paths:
        print('没有输入文件', file=sys.stderr)
        return 2

    defs = {}     # (ne, def_obj, def_key_param, key_val) -> set(name_val)
    refs = []     # (rule_idx, ne, ref_key_val, ref_name_val)
    meid2name = {}
    for path in paths:
        data = open(path, 'rb').read()
        ne = None
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r; \t')
            m = USE_ME.search(line)
            if m:
                ne = meid2name.get(int(m.group(1)))
                continue
            m = MML.match(line)
            if not m:
                continue
            if m.group(2) == b'ME' and m.group(1) == b'ADD':
                # `ADD ME:MEID=281,MENAME="AGCF63",METYPE=AGCF`（已 rstrip 掉分号）
                meid = name = None
                for k, v in split_params(line[m.end():]):
                    if k == 'MEID':
                        meid = int(unq(v))
                    elif k == 'MENAME':
                        name = gbk(unq(v))
                if meid is not None and name is not None:
                    meid2name[meid] = name
                continue
            if ne is None:
                continue
            obj = m.group(2).decode()
            ps = {}
            for k, v in split_params(line[m.end():]):
                ps[k] = unq(v)
            for ri, (robj, rk, rn, dobs, dk, dn) in enumerate(RULES):
                if obj == robj and rk in ps and rn in ps:
                    refs.append((ri, ne, ps[rk], gbk(ps[rn])))
                if obj in dobs and dk in ps and dn in ps:
                    defs.setdefault((ne, dobs, dk, ps[dk]), set()).add(gbk(ps[dn]))

    bad = collections.Counter()
    ex = []
    for (ri, n, k, v) in refs:
        robj, rk, rn, dobs, dk, dn = RULES[ri]
        d = defs.get((n, dobs, dk, k))
        if d is None:
            bad['%s.%s 未找到定义' % (robj, rk)] += 1
        elif v not in d:
            bad['%s.%s 与 %s.%s 不一致' % (robj, rn, '+'.join(dobs), dn)] += 1
            if len(ex) < 10:
                ex.append((n, robj, k, v, sorted(d)))

    print('引用名校验：%d 条引用 × %d 个文件' % (len(refs), len(paths)))
    if not bad:
        print('全部一致 ✓（名称列 ≡ f(数值键)，0 漂移）')
        return 0
    for k, v in bad.most_common():
        print('✗', k, ':', v)
    for e in ex:
        print('  样例:', e)
    return 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
