# -*- coding: utf-8 -*-
"""N7TKC 的 3 个未声明参数（MD/SEN/TIDPFX）能不能按组常数化——决定 `Circuit{11 B}` 是否装得下。"""
import glob
import sys
from collections import Counter, defaultdict

import probe_n7 as p


def main(paths):
    rows = p.load(paths)
    tkc = [(ne, kv) for ne, moc, kv in rows if moc == 'N7TKC']
    g = defaultdict(list)
    for ne, kv in tkc:
        g[(ne, kv.get(b'TG'))].append(kv)
    for name in (b'MD', b'SEN', b'TIDPFX'):
        const_groups = sum(1 for v in g.values() if len({x.get(name) for x in v}) == 1)
        present = sum(1 for _, kv in tkc if name in kv)
        vals = Counter(x.get(name) for v in g.values() for x in v)
        print(f'{name.decode():7} 出现 {present}/{len(tkc)} 行；组内恒定 {const_groups}/{len(g)} 组；distinct {len(vals)}')
        print('        取值 top', vals.most_common(5))
        # 只看不恒定的组
        bad = [(k, len({x.get(name) for x in v}), {x.get(name) for x in v}) for k, v in g.items()
               if len({x.get(name) for x in v}) > 1]
        if bad:
            print('        不恒定组', bad[:3])
    # 组内恒定但组间不同的情况：能否放到 N7TG 行上
    print()
    for name in (b'MD', b'SEN', b'TIDPFX'):
        per_group = {k: {x.get(name) for x in v} for k, v in g.items()}
        print(name.decode(), '各组取值集合大小', Counter(len(s) for s in per_group.values()))
    return 0


if __name__ == '__main__':
    files = []
    for a in sys.argv[1:]:
        files.extend(sorted(glob.glob(a)) if any(c in a for c in '*?[') else [a])
    sys.exit(main(files))
