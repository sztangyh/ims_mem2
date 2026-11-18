# -*- coding: utf-8 -*-
"""探查：PRATKC / N7TKC 的形态是否支持 D53 的两种无损表示。只读原文，不做任何假设。"""
import glob
import re
import sys
from collections import Counter, defaultdict

USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ADD_ME = re.compile(rb'^ADD ME:(.*);\s*$')


def split_params(body):
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


def num(v):
    return int(v) if v is not None and v.isdigit() else None


def load(paths):
    """→ (ne_name_by_meid, [(ne, moc, {param: bytes})...])"""
    rows = []
    for path in paths:
        data = open(path, 'rb').read()
        names = {}
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r \t')
            m = ADD_ME.match(line)
            if m:
                kv = dict(split_params(m.group(1)))
                if b'MEID' in kv and b'MENAME' in kv and kv[b'MEID'].isdigit():
                    names[int(kv[b'MEID'])] = kv[b'MENAME'].decode('gbk', 'replace')
        meid = None
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r \t')
            m = USE_ME.match(line)
            if m:
                meid = int(m.group(1))
                continue
            if meid is None:
                continue
            if line.startswith((b'ADD ', b'MOD ', b'SET ')):
                sp = line.find(b' ')
                colon = line.find(b':', sp)
                if colon < 0:
                    continue
                moc = line[sp + 1:colon].upper().decode('ascii', 'replace')
                if not line.endswith(b';'):
                    continue
                body = line[colon + 1:-1]
                rows.append((names.get(meid, '?'), moc, dict(split_params(body))))
    return rows


def main(paths):
    rows = load(paths)
    print('total mml rows', len(rows))

    # ---------- PRATKC ----------
    tkc = [(ne, kv) for ne, moc, kv in rows if moc == 'PRATKC']
    tg = {(ne, num(kv.get(b'TG'))): kv for ne, moc, kv in rows if moc == 'PRATG'}
    lnk = {(ne, num(kv.get(b'PLN'))): kv for ne, moc, kv in rows if moc == 'PRALNK'}
    print()
    print('=== PRATKC ===')
    print('raw rows', len(tkc))
    keys = set()
    for ne, kv in tkc:
        keys.add((ne, num(kv.get(b'MN')), num(kv.get(b'TG')), num(kv.get(b'SC'))))
    print('distinct (ne,mn,tg,sc)', len(keys))

    # 按 (ne, tg) 分组
    g = defaultdict(list)
    for ne, kv in tkc:
        g[(ne, num(kv.get(b'TG')))].append(kv)
    print('groups (ne,tg)', len(g))
    multi_mn = sum(1 for v in g.values() if len({num(kv.get(b'MN')) for kv in v}) > 1)
    print('groups with >1 MN', multi_mn)
    size_hist = Counter(len(v) for v in g.values())
    print('rows-per-group hist (top)', sorted(size_hist.items())[:12])
    # 每组是否 32 的整数倍
    bad32 = [k for k, v in g.items() if len(v) % 32]
    print('groups whose row count not %32', len(bad32), bad32[:5])

    # 每 (ne,mn,tg)：SC 连续性 / EC / TID / CS / TGN / TIDPFX
    gg = defaultdict(list)
    for ne, kv in tkc:
        gg[(ne, num(kv.get(b'MN')), num(kv.get(b'TG')))].append(kv)
    print('groups (ne,mn,tg)', len(gg))
    not_contig = not_ec = not_tid = bad_cs = tgn_diff = pfx_diff = not32 = 0
    samples = []
    for k, v in gg.items():
        if len(v) != 32:
            not32 += 1
            if len(samples) < 5:
                samples.append(('not32', k, len(v)))
        scs = sorted(num(x.get(b'SC')) for x in v)
        if scs != list(range(scs[0], scs[0] + 32)):
            not_contig += 1
            if len(samples) < 8:
                samples.append(('sc', k, scs[:4], scs[-2:]))
        if any(num(x.get(b'EC')) != num(x.get(b'SC')) for x in v):
            not_ec += 1
            if len(samples) < 10:
                samples.append(('ec', k, [(num(x.get(b'SC')), num(x.get(b'EC'))) for x in v[:3]]))
        base = num(v[0].get(b'TID')) - num(v[0].get(b'SC'))
        if any((num(x.get(b'TID')) or 0) - (num(x.get(b'SC')) or 0) != base for x in v):
            not_tid += 1
            if len(samples) < 12:
                samples.append(('tid', k, [(num(x.get(b'SC')), num(x.get(b'TID'))) for x in v[:3]]))
        css = {x.get(b'CS') for x in v}
        if not css <= {b'USE', b'UNU'}:
            bad_cs += 1
            if len(samples) < 14:
                samples.append(('cs', k, css))
        if len({x.get(b'TGN') for x in v}) > 1:
            tgn_diff += 1
            if len(samples) < 16:
                samples.append(('tgn', k, {x.get(b'TGN') for x in v}))
        if len({x.get(b'TIDPFX') for x in v}) > 1:
            pfx_diff += 1
    print('not exactly 32 rows:', not32)
    print('SC not contiguous 32:', not_contig)
    print('EC != SC:', not_ec)
    print('TID not linear with SC:', not_tid)
    print('CS outside {USE,UNU}:', bad_cs)
    print('TGN varies inside group:', tgn_diff)
    print('TIDPFX varies inside group:', pfx_diff)
    for s in samples:
        print('   sample', s)

    # TIDPFX / CS 取值分布
    print('CS values', Counter(kv.get(b'CS') for _, kv in tkc))
    print('TIDPFX values', Counter(kv.get(b'TIDPFX') for _, kv in tkc).most_common(6))

    # PRATKC.TGN vs PRATG.TGN
    same = diff = missing_tg = no_tgn = 0
    for ne, kv in tkc:
        k = (ne, num(kv.get(b'TG')))
        if k not in tg:
            missing_tg += 1
            continue
        a = kv.get(b'TGN')
        b = tg[k].get(b'TGN')
        if a is None:
            no_tgn += 1
        elif a == b:
            same += 1
        else:
            diff += 1
    print('PRATKC.TGN == PRATG.TGN:', same, 'differs:', diff, 'tg not in PRATG:', missing_tg, 'null:', no_tgn)

    # PRATG.LINK <-> PRALNK.PLN
    hit = miss = nolink = 0
    for (ne, t), kv in tg.items():
        l = num(kv.get(b'LINK'))
        if l is None:
            nolink += 1
        elif (ne, l) in lnk:
            hit += 1
        else:
            miss += 1
    print('PRATG.LINK -> PRALNK.PLN hit', hit, 'miss', miss, 'no LINK', nolink)
    used = {(ne, num(kv.get(b'LINK'))) for (ne, _), kv in tg.items()}
    orphan = [k for k in lnk if k not in used]
    print('PRALNK rows not referenced', len(orphan), orphan[:5])
    print('PRALNK raw rows', sum(1 for _, moc, _ in rows if moc == 'PRALNK'),
          'distinct', len(lnk))
    print('PRATG raw rows', sum(1 for _, moc, _ in rows if moc == 'PRATG'), 'distinct', len(tg))
    # PRATG 中 TG 没电路的
    with_circ = {(ne, t) for (ne, t) in g}
    print('PRATG TGs without PRATKC', len([k for k in tg if k not in with_circ]))

    # ---------- N7TKC ----------
    print()
    print('=== N7TKC ===')
    n7 = [(ne, kv) for ne, moc, kv in rows if moc == 'N7TKC']
    n7tg = {(ne, num(kv.get(b'TG'))): kv for ne, moc, kv in rows if moc == 'N7TG'}
    print('raw rows', len(n7))
    print('distinct (ne,mn,sc)', len({(ne, num(kv.get(b'MN')), num(kv.get(b'SC'))) for ne, kv in n7}))
    g7 = defaultdict(list)
    for ne, kv in n7:
        g7[(ne, num(kv.get(b'TG')))].append(kv)
    print('groups (ne,tg)', len(g7), 'N7TG rows', len(n7tg))
    sizes = Counter(len(v) for v in g7.values())
    print('rows per (ne,tg) hist', sorted(sizes.items())[:12])
    span = []
    for k, v in list(g7.items())[:4000]:
        scs = sorted(num(x.get(b'SC')) for x in v)
        span.append((len(v), scs[-1] - scs[0] + 1))
    dense = sum(1 for n, s in span if n == s)
    print('groups where len == sc span (dense):', dense, '/', len(span))
    print('SCIC distinct', len({x.get(b'SCIC') for _, kv in n7 for x in [kv]}))
    print('SCF values', Counter(x.get(b'SCF') for _, kv in n7))
    print('CS values (n7)', Counter(x.get(b'CS') for _, kv in n7).most_common(6))
    print('MN distinct per group >1:', sum(1 for v in g7.values() if len({num(x.get(b'MN')) for x in v}) > 1))
    print('TGN const inside group:', sum(1 for v in g7.values() if len({x.get(b'TGN') for x in v}) == 1), '/', len(g7))
    n7_ok = sum(1 for v in g7.values() if all(num(x.get(b'EC')) == num(x.get(b'SC')) for x in v))
    print('EC==SC everywhere (groups):', n7_ok)
    # N7TKC.TGN vs N7TG.TGN
    s = d = m = 0
    for ne, kv in n7:
        k = (ne, num(kv.get(b'TG')))
        if k not in n7tg:
            m += 1
        elif kv.get(b'TGN') == n7tg[k].get(b'TGN'):
            s += 1
        else:
            d += 1
    print('N7TKC.TGN == N7TG.TGN:', s, 'differs', d, 'tg not in N7TG', m)
    print('N7TKC TGs without N7TG:', len([k for k in g7 if k not in n7tg]))
    return 0


if __name__ == '__main__':
    files = []
    for p in sys.argv[1:]:
        files.extend(sorted(glob.glob(p)) if any(c in p for c in '*?[') else [p])
    sys.exit(main(files))
