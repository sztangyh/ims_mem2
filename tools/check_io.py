# -*- coding: utf-8 -*-
"""M1 的 IO 与金标准逐键逐列对拍：PraLink / ClrdsnGroup / Subscriber。

用法：
    python tools/check_io.py --io PraLink [--io-file temp/io_pralink.ndjson]

金标准 = `.temp/bridge_<表>.ndjson`（旧桥接层输出）。三类比较模式：
  direct      —— 键完全对应，逐列比
  collapse    —— 金标准需先按 (ne,dsp) 收敛（最长 cli）再比
  asbr        —— 金标准来自 3 张分表（按 DID 分流），按 (ne_id, nsn) 合并后比
"""
import collections
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN_DIR = r'C:\dev\tryit\.temp'
EXEMPT = {'cur_from', 'diff'}

IOS = {
    'PraLink': {
        'golden': ['pratg'],
        'mode': 'direct',
        'key': ['ne_id', 'tg'],
        'fields': {'tg': 'tg', 'binifid': 'BINIFID', 'lks': 'LKS', 'mgw_mid': 'MGW'},
        'out': os.path.join(ROOT, 'temp', 'io_pralink.ndjson'),
        'allow_missing': [('2', '254'), ('3', '347')],  # PRALNK 孤儿，记录在案
        'allow_missing_note': 'PRALNK 2 条孤儿没有 PRATG 行（D53 的已知例外）',
    },
    'ClrdsnGroup': {
        'golden': ['clrdsn'],
        'mode': 'collapse',
        'key': ['ne_id', 'dsp'],
        'fields': {'dsp': 'dsp', 'func': 'func', 'min': 'min', 'max': 'max',
                   'cli': 'cli', 'members': None},
        'collapse_key': 'dsp',
        'longest': 'cli',
        'out': os.path.join(ROOT, 'temp', 'io_clrdsngroup.ndjson'),
    },
    'Subscriber': {
        'golden': ['asbr_mgw', 'asbr_trunk', 'asbr_v5'],
        'mode': 'asbr',
        'key': ['ne_id', 'nsn'],
        'fields': {'nsn': 'nsn', 'mn': 'mn', 'eid': 'eid', 'tid': 'tid',
                   'isdn': 'isdn', 'tg': 'tg', 'v5iid': 'v5iid', 'l3addr': 'l3addr',
                   'scc': 'scc', 'netid': 'netid', 'netinfo': 'netinfo'},
        'out': os.path.join(ROOT, 'temp', 'io_subscriber.ndjson'),
    },
    'Rtana': {
        'golden': ['rtana'],
        'mode': 'direct',
        'key': ['ne_id', 'rsc', 'r'],
        'fields': {'rsc': 'rsc', 'rssc': 'rssc', 'ran': 'ran', 'r': 'r'},
        'out': os.path.join(ROOT, 'temp', 'io_rtana.ndjson'),
    },
    'Rt': {
        'golden': ['rt'],
        'mode': 'direct',
        'key': ['ne_id', 'r'],
        'fields': {'r': 'r', 'perc': 'perc', 'perccfg': 'perccfg',
                   'rn': 'rn', 'srst': 'srst'},
        'out': os.path.join(ROOT, 'temp', 'io_rt.ndjson'),
    },
    'Srt': {
        'golden': ['srt'],
        'mode': 'direct',
        'key': ['ne_id', 'src'],
        'fields': {'src': 'src', 'o': 'o', 'srn': 'srn'},
        'out': os.path.join(ROOT, 'temp', 'io_srt.ndjson'),
    },
    'Tgsrt': {
        'golden': ['tgsrt'],
        'mode': 'direct',
        'key': ['ne_id', 'tg', 'src'],
        'fields': {'tg': 'tg', 'src': 'src', 'srn': 'srn'},
        'out': os.path.join(ROOT, 'temp', 'io_tgsrt.ndjson'),
    },
    'V5Access': {
        'golden': ['v5i', 'v5tg'],
        'mode': 'join2',
        'key': ['ne_id', 'v5iid'],
        'fields': {'v5iid': 'v5iid', 'tgid': 'TGID', 'mid': 'MID',
                   'pnum': 'PNUM', 'snum': 'SNUM', 'pstnnum': 'PSTNNUM'},
        'out': os.path.join(ROOT, 'temp', 'io_v5access.ndjson'),
        'allow_missing': [('1', None), ('2', None)],
        'allow_missing_note': '金标准 v5i 有 2 行 v5iid 为空，无法构成对象（io_spec 要求键非空）',
    },
    'Transform': {
        'golden': ['tgld'],
        'mode': 'direct',
        'key': ['ne_id', 'cli'],
        'fields': {'cli': 'cli', 'bpn': 'bpn', 'ei': 'ei', 'ori': 'ori',
                   'rdi': 'rdi', 'ri': 'ri'},
        'out': os.path.join(ROOT, 'temp', 'io_transform.ndjson'),
    },
    'SrtAttr': {
        'golden': ['srtattr'],
        'mode': 'direct',
        'key': ['ne_id', 'r', 'sr'],
        'fields': {'r': 'r', 'sr': 'sr', 'srtp': 'srtp', 'srn': 'srn',
                   'psr': 'psr', 'cst': 'cst'},
        'out': os.path.join(ROOT, 'temp', 'io_srtattr.ndjson'),
    },
    'N7tg': {
        'golden': ['n7tg'],
        'mode': 'direct',
        'key': ['ne_id', 'tg'],
        'fields': {'tg': 'tg', 'csc': 'csc', 'ct': 'ct', 'di': 'di', 'eid': 'eid',
                   'icr': 'icr', 'ioi': 'ioi', 'isbf': 'isbf', 'isdp': 'isdp',
                   'mtg': 'mtg', 'nicf': 'nicf', 'npcld': 'npcld', 'ocr': 'ocr',
                   'rival': 'rival', 'sdpc': 'sdpc', 'sopc': 'sopc', 'src': 'src',
                   'srm': 'srm', 'tgms': 'tgms', 'tgn': 'tgn', 'kc': 'kc'},
        'out': os.path.join(ROOT, 'temp', 'io_n7tg.ndjson'),
    },
    'Ne': {
        'golden': [],
        'mode': 'no_golden',
        'key': ['ne_id'],
        'expect_rows': 6,
        'out': os.path.join(ROOT, 'temp', 'io_ne.ndjson'),
    },
}

def load(path):
    rows = []
    with open(path, encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def norm(v):
    if v is None:
        return None
    if isinstance(v, bool):
        return '1' if v else '0'
    if isinstance(v, (int, float)):
        return str(int(v)) if float(v).is_integer() else str(v)
    return str(v)


def collapse_golden(rows, key_col, longest_col):
    """同 (ne_id,dsp) 取最长 cli 那行做代表（同长取 cli 字典序小者）。"""
    best = {}
    clis = collections.defaultdict(set)
    for r in rows:
        k = (norm(r.get('ne_id')), norm(r.get(key_col)))
        c = norm(r.get(longest_col)) or ''
        clis[k].add(c)
        cur = best.get(k)
        if cur is None or len(c) > len(norm(cur.get(longest_col)) or '') or (
                len(c) == len(norm(cur.get(longest_col)) or '') and c < (norm(cur.get(longest_col)) or '')):
            best[k] = r
    return best, {k: len(v) for k, v in clis.items()}


def main():
    which = None
    io_file = None
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == '--io':
            which = args[i + 1]
            i += 2
        elif args[i] == '--io-file':
            io_file = args[i + 1]
            i += 2
        else:
            i += 1
    if which == 'Ne':
        pass
    if which not in IOS:
        print(f'用法: check_io.py --io <{"|".join(IOS)}>', file=sys.stderr)
        return 2
    cfg = IOS[which]
    io_path = io_file or cfg['out']
    if not os.path.exists(io_path):
        print(f'缺 {io_path}（先跑 cargo run --example io_m1 -- --io {which}）', file=sys.stderr)
        return 2

    got = {}
    for r in load(io_path):
        k = tuple(norm(r.get(c)) for c in cfg['key'])
        got[k] = r

    if cfg['mode'] == 'no_golden':
        n = len(got)
        want_n = cfg.get('expect_rows')
        ok = (want_n is None) or (n == want_n)
        print(f'{which}: io {n} 个（期望 {want_n}）')
        print(('ok ' if ok else 'x ') + f'{which} 行数校验')
        return 0 if ok else 1

    gold_rows = []
    for name in cfg['golden']:
        p = os.path.join(GOLDEN_DIR, f'bridge_{name}.ndjson')
        if not os.path.exists(p):
            print(f'缺金标准 {p}', file=sys.stderr)
            return 2
        gold_rows.extend(load(p))

    extra_members = None
    colsets = {}
    if cfg['mode'] == 'collapse':
        want, counts = collapse_golden(gold_rows, cfg['collapse_key'], cfg['longest'])
        extra_members = counts
    elif cfg['mode'] == 'asbr':
        want = {}
        for name in cfg['golden']:
            p = os.path.join(GOLDEN_DIR, f'bridge_{name}.ndjson')
            for r in load(p):
                k = tuple(norm(r.get(c)) for c in cfg['key'])
                want[k] = r
                colsets[k] = set(r.keys())
    else:
        want = {}
        for r in gold_rows:
            k = tuple(norm(r.get(c)) for c in cfg['key'])
            want[k] = r

    print(f'{which}: io {len(got)} 个 / 金标准 {len(want)} 个')
    allow = set(cfg.get('allow_missing', []))
    extra = sorted(set(got) - set(want))
    missing_all = sorted(set(want) - set(got))
    missing = [k for k in missing_all if k not in allow]
    allowed_missing = [k for k in missing_all if k in allow]
    if allowed_missing:
        print(f'  已登记例外 {len(allowed_missing)} 个键: {cfg.get("allow_missing_note", "")}')
    if extra:
        print(f'  多出 {len(extra)} 个键，例: {extra[:4]}')
    if missing:
        print(f'  缺少 {len(missing)} 个键，例: {missing[:4]}')

    per_col = collections.Counter()
    samples = collections.defaultdict(list)
    for k in sorted(set(got) & set(want)):
        a, b = got[k], want[k]
        for fname, gcol in cfg['fields'].items():
            if gcol is None:
                continue
            if cfg['mode'] == 'asbr':
                cs = colsets.get(k, set())
                if gcol not in cs:
                    continue  # 该变体的金标准表里没有这一列（如 asbr_trunk 无 netid）
            va, vb = norm(a.get(fname)), norm(b.get(gcol))
            if va != vb:
                per_col[fname] += 1
                if len(samples[fname]) < 2:
                    samples[fname].append((k, vb, va))
    total = sum(per_col.values())
    print(f'逐列对拍（{len([v for v in cfg["fields"].values() if v])} 列）：差异 {total}')
    for c in sorted(per_col, key=lambda x: -per_col[x]):
        k, wv, gv = samples[c][0]
        print(f'  {c:9} {per_col[c]:>6} 处   例 {k}: 金标准={wv!r} 本仓={gv!r}')

    if which == 'ClrdsnGroup' and extra_members:
        bad = [(k, norm(got[k].get('members')), extra_members.get(k))
               for k in set(got) & set(extra_members)
               if norm(got[k].get('members')) != str(extra_members[k])]
        print(f'members 校验：{len(bad)} 处不符' + (f'，例 {bad[:3]}' if bad else ''))
        if bad:
            total += len(bad)

    if total == 0 and not extra and not missing:
        print(f'ok {which} == 金标准（0 差异）')
        return 0
    print(f'x {which} 与金标准有差异')
    return 1


if __name__ == '__main__':
    sys.exit(main())
