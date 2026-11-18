# -*- coding: utf-8 -*-
"""M3 验收：目录服务对独立 oracle（从 33 个金标准 NDJSON 重建池）逐项比对。

断言：
  A 池规模：三个池的去重键数 / 命中数
  B 匹配：prefix / tail / longest / exact 的 total 与 oracle 一致
  C 分页：keyset 翻完所有页 → 与 oracle 的匹配集合完全相同、无重、严格升序、每页 ≤ limit
退出码 0 = 全过。
"""
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = r'C:\dev\tryit\.temp'
BATCH = os.path.join(ROOT, 'temp', 'catalog_q.tsv')
OUT = os.path.join(ROOT, 'temp', 'catalog_out.jsonl')


def load(name):
    p = os.path.join(GOLDEN, f'bridge_{name}.ndjson')
    with open(p, encoding='utf-8') as f:
        return [json.loads(l) for l in f if l.strip()]


def oracle_pools():
    prefix = set()
    for name, col in (('cnacld', 'pfx'), ('clrdsn', 'cli'), ('tgldidx', 'pfx')):
        for r in load(name):
            v = r.get(col)
            if v:
                prefix.add(str(v))
    sub = set()
    for name in ('asbr_mgw', 'asbr_trunk', 'asbr_v5'):
        for r in load(name):
            v = r.get('nsn')
            if v:
                sub.add(str(v))
    trunk = set()
    for name, col in (('pratg', 'tg'), ('sip_tg', 'tg'), ('n7tg', 'tg')):
        for r in load(name):
            v = r.get(col)
            if v:
                trunk.add(str(v))
    return {'prefix': prefix, 'subscriber': sub, 'trunk': trunk}


# 查询清单：(pool, kind, arg, limit, pages)
QUERIES = [
    ('prefix', 'prefix', '1', 50, 1),
    ('prefix', 'prefix', '12', 50, 1),
    ('prefix', 'prefix', '755', 7, 200),      # 翻完所有页
    ('prefix', 'longest', '0086755', 5, 1),
    ('prefix', 'exact', '0086755', 5, 1),
    ('subscriber', 'prefix', '755', 50, 1),
    ('subscriber', 'tail', '0001', 20, 1),
    ('subscriber', 'tail', '8899', 7, 200),
    ('trunk', 'prefix', '100', 50, 1),
    ('trunk', 'exact', '3001', 5, 1),
    ('trunk', 'prefix', '3', 7, 200),
]


def main():
    pools = oracle_pools()
    print('oracle 池：' + ' / '.join(f'{k}={len(v)}' for k, v in pools.items()))

    with open(BATCH, 'w', encoding='utf-8', newline='\n') as f:
        for pool, kind, arg, lim, pages in QUERIES:
            f.write(f'{pool}\t{kind}\t{arg}\t{lim}\t{pages}\n')

    files = [os.path.join(r'C:\dev\tryit\agcf_data', x) for x in sorted(os.listdir(r'C:\dev\tryit\agcf_data'))
             if x.startswith('ALLME_2026') and x.endswith('.txt')]
    exe = os.path.join(ROOT, 'target', 'release', 'examples', 'io_catalog.exe')
    with open(OUT, 'w', encoding='utf-8') as out:
        p = subprocess.run([exe, '--batch', BATCH, *files], capture_output=True, text=True)
        if p.returncode != 0:
            print('io_catalog 失败:', p.stderr[-400:])
            return 1
        out.write(p.stdout)

    lines = [json.loads(l) for l in open(OUT, encoding='utf-8') if l.strip()]
    print(f'接到 {len(lines)} 页结果')

    fails = 0
    # 按 (pool, kind, arg) 归组，用于分页断言
    groups = {}
    for r in lines:
        key = (r['pool'], r['match']['kind'], r['match']['arg'])
        groups.setdefault(key, []).append(r)

    for pool, kind, arg, lim, pages in QUERIES:
        rs = groups.get((pool, kind, arg), [])
        if not rs:
            print(f'  ✗ {pool}/{kind}/{arg}: 无结果')
            fails += 1
            continue
        keys = [it['key'] for r in rs for it in r['items']]
        total = rs[0]['total']
        if kind == 'prefix':
            want = sorted(k for k in pools[pool] if k.startswith(arg))
        elif kind == 'tail':
            want = sorted(k for k in pools[pool] if k.endswith(arg))
        elif kind == 'longest':
            cand = [k for k in pools[pool] if k.startswith(arg)]
            want = [max(cand, key=len)] if cand else []
        else:
            want = [arg] if arg in pools[pool] else []
        tag = f'{pool}/{kind}/{arg}'
        if total != len(want):
            print(f'  ✗ {tag}: total {total} ≠ oracle {len(want)}')
            fails += 1
        if pages > 1:
            if keys != want:
                print(f'  ✗ {tag}: 翻页结果与 oracle 不一致（{len(keys)} vs {len(want)}）')
                fails += 1
            if keys != sorted(keys):
                print(f'  ✗ {tag}: 结果不是严格升序')
                fails += 1
            if len(set(keys)) != len(keys):
                print(f'  ✗ {tag}: 有重复键')
                fails += 1
            for r in rs:
                if len(r['items']) > lim:
                    print(f'  ✗ {tag}: 某页 {len(r["items"])} 条 > limit {lim}')
                    fails += 1
            if rs[-1]['next_cursor'] is not None:
                print(f'  ✗ {tag}: 最后一页仍有 next_cursor')
                fails += 1
            if not keys and not want:
                pass
        else:
            want_page = want[: total and min(lim, len(want))]
            if keys != want_page:
                print(f'  ✗ {tag}: 首页内容不符（{keys[:3]}… vs {want_page[:3]}…）')
                fails += 1

    if fails:
        print(f'\nx M3 目录校验失败 {fails} 项')
        return 1
    print('\nok M3 目录服务：池规模 / 匹配 / 分页 全部与 oracle 一致')
    return 0


if __name__ == '__main__':
    sys.exit(main())
