# -*- coding: utf-8 -*-
"""N7TKC / N7TG 形态探查（D53 步骤 2 的前置度量）：能否用稀疏 `Circuit{mn,sc,ec,tid,scic,flags}` 无损表示。"""
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


def load(paths):
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
            if meid is None or not line.startswith((b'ADD ', b'MOD ', b'SET ')) or not line.endswith(b';'):
                continue
            sp = line.find(b' ')
            colon = line.find(b':', sp)
            if colon < 0:
                continue
            moc = line[sp + 1:colon].upper().decode('ascii', 'replace')
            rows.append((names.get(meid, '?'), moc, dict(split_params(line[colon + 1:-1]))))
    return rows


def main(paths):
    rows = load(paths)
    tkc = [(ne, kv) for ne, moc, kv in rows if moc == 'N7TKC']
    tg = {(ne, kv.get(b'TG')): kv for ne, moc, kv in rows if moc == 'N7TG'}
    print('N7TKC rows', len(tkc), ' N7TG rows', len(tg))
    print('N7TKC 键 (ne,mn,sc) 去重', len({(ne, kv.get(b'MN'), kv.get(b'SC')) for ne, kv in tkc}))
    print('CS 取值', Counter(kv.get(b'CS') for _, kv in tkc))
    print('SCF 取值', Counter(kv.get(b'SCF') for _, kv in tkc))
    print('参数名集合', sorted({k.decode() for _, kv in tkc for k in kv}))
    g = defaultdict(list)
    for ne, kv in tkc:
        g[(ne, kv.get(b'TG'))].append(kv)
    print('组数 (ne,tg)', len(g))
    print('每组条数', sorted(Counter(len(v) for v in g.values()).items()))
    multi_mn = sum(1 for v in g.values() if len({x.get(b'MN') for x in v}) > 1)
    print('组内 MN 不唯一的组数', multi_mn)
    # tgn 是否与 N7TG.tgn 一致
    same = diff = miss = 0
    for ne, kv in tkc:
        k = (ne, kv.get(b'TG'))
        if k not in tg:
            miss += 1
        elif kv.get(b'TGN') == tg[k].get(b'TGN'):
            same += 1
        else:
            diff += 1
    print('N7TKC.TGN == N7TG.TGN:', same, 'differs', diff, 'tg 不在 N7TG', miss)
    # 组内 tgn 是否唯一
    print('组内 TGN 不唯一的组数', sum(1 for v in g.values() if len({x.get(b'TGN') for x in v}) > 1))
    # EC 与 SC 关系 / TID
    print('EC != SC 的行数', sum(1 for _, kv in tkc if kv.get(b'EC') != kv.get(b'SC')))
    print('EC 缺省的行数', sum(1 for _, kv in tkc if b'EC' not in kv))
    print('TID 缺省的行数', sum(1 for _, kv in tkc if b'TID' not in kv))
    print('SCIC 缺省的行数', sum(1 for _, kv in tkc if b'SCIC' not in kv))
    print('SC 的最大值', max(int(kv[b'SC']) for _, kv in tkc if (kv.get(b'SC') or b'').isdigit()))
    print('TID 的最大值', max(int(kv[b'TID']) for _, kv in tkc if (kv.get(b'TID') or b'').isdigit()))
    # 32 的整数倍？（N7 不要求，这里是记录）
    print('组条数 %32 == 0 的组数', sum(1 for v in g.values() if len(v) % 32 == 0), '/', len(g))
    # 稀疏度：组条数 vs SC 跨度
    spans = []
    for v in g.values():
        scs = sorted(int(x[b'SC']) for x in v if (x.get(b'SC') or b'').isdigit())
        spans.append((len(scs), scs[-1] - scs[0] + 1))
    print('组条数/跨度 样例', spans[:8])
    print('dense 组数', sum(1 for n, s in spans if n == s), '/', len(spans))
    # N7TG 有电路但没有 TKC 的
    print('N7TG 没有 N7TKC 的', len([k for k in tg if k not in g]))
    return 0


if __name__ == '__main__':
    files = []
    for p in sys.argv[1:]:
        files.extend(sorted(glob.glob(p)) if any(c in p for c in '*?[') else [p])
    sys.exit(main(files))
