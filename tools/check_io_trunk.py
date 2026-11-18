# -*- coding: utf-8 -*-
"""Trunk IO 与金标准逐键逐列对拍（spec/acceptance.md M1）。

用法：
    python tools/check_io_trunk.py [--io temp/trunk_io.ndjson] [--golden <bridge_trunk.ndjson>]

豁免：cur_from / diff（见 acceptance.md M1 #2）。
退出码 0 = 0 差异；1 = 有差异。
"""
import collections
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IO = os.path.join(ROOT, 'temp', 'trunk_io.ndjson')
GOLDEN = r'C:\dev\tryit\.temp\bridge_trunk.ndjson'
EXEMPT = {'cur_from', 'diff'}

args = sys.argv[1:]
i = 0
while i < len(args):
    if args[i] == '--io':
        IO = args[i + 1]
        i += 2
    elif args[i] == '--golden':
        GOLDEN = args[i + 1]
        i += 2
    else:
        i += 1


def load(path, tag):
    rows = {}
    with open(path, encoding='utf-8') as f:
        for n, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            o = json.loads(line)
            ne = o.get('ne_id')
            tg = o.get('tg')
            if tg is None:
                print(f'  {tag} 第 {n} 行缺 tg', file=sys.stderr)
                continue
            rows[(ne, tg)] = o
    return rows


def norm(v):
    """统一成字符串比较（金标准的数值列是 number）。"""
    if v is None:
        return None
    if isinstance(v, bool):
        return '1' if v else '0'
    if isinstance(v, (int, float)):
        return str(int(v)) if float(v).is_integer() else str(v)
    return str(v)


def main():
    if not os.path.exists(IO):
        print(f'缺 {IO}（先跑 cargo run --example io_trunk）', file=sys.stderr)
        return 2
    if not os.path.exists(GOLDEN):
        print(f'缺金标准 {GOLDEN}', file=sys.stderr)
        return 2
    got = load(IO, 'io')
    want = load(GOLDEN, 'golden')
    print(f'io {len(got)} 个 / 金标准 {len(want)} 个')

    extra = sorted(set(got) - set(want))
    missing = sorted(set(want) - set(got))
    if extra:
        print(f'  多出 {len(extra)} 个键，例: {extra[:5]}')
    if missing:
        print(f'  缺少 {len(missing)} 个键，例: {missing[:5]}')

    cols = sorted({k for o in want.values() for k in o} - EXEMPT)
    per_col = collections.Counter()
    samples = collections.defaultdict(list)
    for key in sorted(set(got) & set(want)):
        a, b = got[key], want[key]
        for c in cols:
            va, vb = norm(a.get(c)), norm(b.get(c))
            if va != vb:
                per_col[c] += 1
                if len(samples[c]) < 3:
                    samples[c].append((key, vb, va))

    total = sum(per_col.values())
    print(f'\n逐列对拍（{len(cols)} 列，豁免 {sorted(EXEMPT)}）：差异 {total}')
    for c in sorted(per_col, key=lambda x: -per_col[x]):
        k, wv, gv = samples[c][0]
        print(f'  {c:10} {per_col[c]:>6} 处   例 ne={k[0]} tg={k[1]}: 金标准={wv!r} 本仓={gv!r}')

    if total == 0 and not extra and not missing:
        print('\nok Trunk IO == 金标准（0 差异）')
        return 0
    print('\nx Trunk IO 与金标准有差异')
    return 1


if __name__ == '__main__':
    sys.exit(main())
