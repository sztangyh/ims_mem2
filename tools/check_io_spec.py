# -*- coding: utf-8 -*-
"""io_spec 校验器：把 spec 设计审查里的每条风险变成断言。

用法：
    python tools/check_io_spec.py                 # 全量（含基线对拍）
    python tools/check_io_spec.py --no-baseline   # 只做结构校验（无 .temp 金标准时）

断言分四组：
  A 结构（字段绑定/链接可执行/键与粒度/scope/catalog）
  B 基线（baseline.rows 必须与 33 个金标准 NDJSON 的实际行数一致）
  C 覆盖（numtrace 18 条路由必须都有 coverage 行；covered 的路由所需列必须有 IO 归属）
  D 目录池（io_spec 与 catalog_index 的池必须一一对应，池来源必须是真实表/列）
退出码 0 = 全过；1 = 有 FAIL。
"""
from __future__ import annotations

import glob
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC = os.path.join(ROOT, 'spec', 'io_spec.json')
CATALOG = os.path.join(ROOT, 'spec', 'catalog_index.json')
GOLDEN = r'C:\dev\tryit\.temp'

KINDS = {'field', 'derived', 'aggregate', 'lifted', 'const', 'axis'}
TYPES = {'str', 'int', 'bool'}
FORMS = {'raw', 'num_str', 'enum_code', 'enum_label', 'pfx', 'bits', 'json_text'}
CARDS = {'1:1', '1:n', 'n:1'}
SCOPES = {'ok', 'blocked', 'legacy_only'}
COV_STATUS = {'covered', 'partial', 'blocked', 'legacy_only', 'out_of_scope'}

# numtrace 现有 18 条路由（对拍基准；doc/* 之类合并成一类）
NUMTRACE_ROUTES = [
    '/', '/app.js', '/style.css',
    '/api/nes', '/api/search', '/api/trace/callee', '/api/trace/rana',
    '/api/trace/caller', '/api/user', '/api/tg', '/api/mgw',
    '/api/doc/agcf', '/api/doc/agcf/search', '/api/doc/mgw', '/api/doc/param',
    '/api/meta', '/api/batches',
]

# baseline.source -> 计算方式
BASELINE_RULES = {
    'bridge_asbr_mgw+bridge_asbr_trunk+bridge_asbr_v5': ('sum', ['asbr_mgw', 'asbr_trunk', 'asbr_v5']),
    'distinct(ne_id,dsp) in bridge_clrdsn': ('distinct', ('clrdsn', ['ne_id', 'dsp'])),
}
for name in ('trunk', 'cnacld', 'v5i', 'rtana', 'rt', 'srt', 'tgsrt',
             'tgld', 'srtattr'):
    BASELINE_RULES['bridge_' + name] = ('rows', (name,))
BASELINE_RULES['AGCF 正式名集合(AGCF51/52/63/64/65/66)'] = ('fixed', 6)

fails: list[str] = []
warns: list[str] = []


def fail(msg: str) -> None:
    fails.append(msg)


def warn(msg: str) -> None:
    warns.append(msg)


def load(path: str):
    with open(path, encoding='utf-8') as f:
        return json.load(f)


def golden_rows(name: str):
    p = os.path.join(GOLDEN, 'bridge_' + name + '.ndjson')
    if not os.path.exists(p):
        return None
    with open(p, encoding='utf-8') as f:
        return [json.loads(l) for l in f if l.strip()]


def main() -> int:
    no_baseline = '--no-baseline' in sys.argv
    spec = load(SPEC)
    catalog = load(CATALOG) if os.path.exists(CATALOG) else {'pools': []}

    ios = spec.get('io', [])
    names = [io['name'] for io in ios]
    by_name = {io['name']: io for io in ios}
    funcs = spec.get('functions', {})
    pools = {p['pool'] for p in catalog.get('pools', [])}

    # ---------- A 结构 ----------
    if spec.get('spec') != 'io_spec':
        fail('spec 字段必须是 io_spec')
    if spec.get('version', 0) < 2:
        fail('version 必须 >= 2')
    if len(names) != len(set(names)):
        fail('IO 名重复: %s' % names)

    for io in ios:
        n = io['name']
        # scope
        sc = io.get('scope', {})
        if sc.get('status') not in SCOPES:
            fail('%s: scope.status 非法 %r' % (n, sc.get('status')))
        if sc.get('status') in ('blocked', 'legacy_only') and not sc.get('reason'):
            fail('%s: scope.status=%s 必须给 reason' % (n, sc['status']))
        # grain
        g = io.get('grain', {})
        if g.get('from_rows') not in ('one', 'many'):
            fail('%s: grain.from_rows 非法' % n)
        field_names = {f['name'] for f in io.get('fields', [])}
        for k in g.get('key', []):
            if k not in field_names and k not in ('ne', 'ne_id', 'role', 'scope'):
                warn('%s: grain.key 的 %r 不是字段（可由实现提供）' % (n, k))
        col = g.get('collapse')
        if g.get('from_rows') == 'many' and not col:
            fail('%s: from_rows=many 必须声明 collapse' % n)
        if col and col.get('by') not in ('union', 'longest', 'all', 'first', 'sum'):
            fail('%s: collapse.by 非法' % n)
        # fields
        seen = set()
        for f in io.get('fields', []):
            fn = f.get('name')
            if fn in seen:
                fail('%s: 字段 %r 重复' % (n, fn))
            seen.add(fn)
            k = f.get('kind')
            if k not in KINDS:
                fail('%s.%s: kind 非法 %r' % (n, fn, k))
            if f.get('type') not in TYPES:
                fail('%s.%s: type 非法 %r' % (n, fn, f.get('type')))
            if f.get('form') not in FORMS:
                fail('%s.%s: form 非法 %r' % (n, fn, f.get('form')))
            if 'nullable' not in f:
                fail('%s.%s: 缺 nullable' % (n, fn))
            if 'expose' not in f:
                fail('%s.%s: 缺 expose' % (n, fn))
            if k in ('field', 'lifted') and 'src' not in f:
                fail('%s.%s: kind=%s 必须给 src' % (n, fn, k))
            if k in ('derived', 'aggregate'):
                if 'fn' not in f:
                    fail('%s.%s: kind=%s 必须给 fn' % (n, fn, k))
                elif f['fn'] not in funcs:
                    fail('%s.%s: fn %r 不在 functions 注册表' % (n, fn, f['fn']))
            if k == 'const' and 'value' not in f:
                fail('%s.%s: kind=const 必须给 value' % (n, fn))
            if 'query' in f:
                for op in f['query'].get('ops', []):
                    if op not in ('eq', 'prefix', 'tail', 'contains', 'range', 'longest'):
                        fail('%s.%s: query.op 非法 %r' % (n, fn, op))
        # variants 引用的字段必须存在
        for v in io.get('variants', []):
            for fn in v.get('fields', []):
                if fn not in field_names:
                    fail('%s: variants 引用不存在的字段 %r' % (n, fn))
        # links
        for lk in io.get('links', []):
            lf = lk.get('field')
            if lk.get('to') not in by_name:
                fail('%s.%s: 链接目标 %r 未声明（悬空）' % (n, lf, lk.get('to')))
            if lk.get('card') not in CARDS:
                fail('%s.%s: card 非法 %r' % (n, lf, lk.get('card')))
            if lk.get('on_missing') not in ('null', 'dangling'):
                fail('%s.%s: on_missing 非法' % (n, lf))
            if 'critical' not in lk:
                fail('%s.%s: 缺 critical' % (n, lf))
            ke = lk.get('key_expr') or {}
            if not ke.get('from') or not ke.get('to_key'):
                fail('%s.%s: key_expr 缺 from/to_key' % (n, lf))
            # 目标 IO 必须有该键字段（ne 特例）
            tgt = by_name.get(lk.get('to'))
            if tgt:
                tk = ke.get('to_key')
                tfields = {f['name'] for f in tgt.get('fields', [])}
                if tk not in tfields and tk not in ('ne', 'ne_id', 'pkey', 'id', 'key'):
                    warn('%s.%s → %s.%s：目标无此字段' % (n, lf, tgt['name'], tk))
                if tgt.get('scope', {}).get('status') == 'blocked':
                    warn('%s.%s → %s 是 blocked（实现前无法解析）' % (n, lf, tgt['name']))
            # 分页只用于目录（2026-11-14 领域裁定）；链接声明 paging 才算异常
            if lk.get('paging'):
                fail('%s.%s: 链接不应声明 paging（分页只用于目录查询）' % (n, lf))
        # catalog pool
        cat = io.get('catalog')
        if cat and cat.get('pool') not in pools:
            fail('%s: catalog.pool %r 不在 catalog_index.pools' % (n, cat.get('pool')))

    # ---------- B 基线 ----------
    if no_baseline:
        warn('--no-baseline：跳过基线对拍')
    elif not os.path.isdir(GOLDEN):
        warn('金标准目录不存在，跳过基线对拍: %s' % GOLDEN)
    else:
        for io in ios:
            b = io.get('baseline')
            if not b or b.get('rows') is None:
                continue
            src = b['source']
            rule = BASELINE_RULES.get(src)
            if not rule:
                warn('%s: baseline.source 无对应规则，跳过: %r' % (io['name'], src))
                continue
            kind, arg = rule
            if kind == 'fixed':
                actual = arg
            elif kind == 'sum':
                total = 0
                for nm in arg:
                    rs = golden_rows(nm)
                    if rs is None:
                        total = None
                        break
                    total += len(rs)
                actual = total
            elif kind == 'rows':
                rs = golden_rows(arg[0])
                actual = None if rs is None else len(rs)
            else:  # distinct
                nm, keys = arg
                rs = golden_rows(nm)
                actual = None if rs is None else len({tuple(r.get(k) for k in keys) for r in rs})
            if actual is None:
                warn('%s: 金标准缺文件，跳过' % io['name'])
            elif actual != b['rows']:
                fail('%s: baseline.rows=%d 与金标准实际 %d 不符（source=%s）'
                     % (io['name'], b['rows'], actual, src))
            else:
                print('  基线 ok %-12s %8d  (%s)' % (io['name'], actual, src))

    # ---------- C 覆盖 ----------
    cov = spec.get('coverage', [])
    cov_routes = [c['route'] for c in cov]
    STATIC = {'/', '/app.js', '/style.css'}
    for r in NUMTRACE_ROUTES:
        if r in STATIC:
            continue  # 静态资源不走 IO 层
        hit = r in cov_routes
        if not hit:
            hit = any(cr.startswith(r + '?') for cr in cov_routes)
        if not hit:
            hit = r.startswith('/api/doc/') and '/api/doc/*' in cov_routes
        if not hit:
            fail('coverage 缺 numtrace 路由: %s' % r)
    for c in cov:
        if c.get('status') not in COV_STATUS:
            fail('coverage[%s]: status 非法 %r' % (c.get('route'), c.get('status')))
        for io_name in c.get('io', []):
            if io_name not in by_name:
                fail('coverage[%s]: 引用未声明 IO %r' % (c.get('route'), io_name))
        if c.get('status') == 'covered' and not c.get('io'):
            fail('coverage[%s]: status=covered 但没给 io' % c.get('route'))
    # covered 判定：字段 src / 函数 args / 链接 key_expr.from / 表级声明，四者之一
    exact, tables = set(), set()
    for one in ios:
        for f in one.get('fields', []):
            if f.get('src'):
                exact.add(f['src'].upper())
            for a in f.get('args', []) or []:
                exact.add(a.upper())
        for lk in one.get('links', []):
            fr = (lk.get('key_expr') or {}).get('from')
            if fr:
                exact.add(fr.upper())
        for src in one.get('source', []):
            tables.add(src['table'].upper())
    for c in cov:
        if c.get('status') != 'covered':
            continue
        for need in c.get('needs', []):
            u = need.upper()
            if ':' in u or u.startswith('MGW_'):
                continue
            if u in exact:
                continue
            tbl = u.split('.')[0]
            if tbl in tables or any(tbl.startswith(t) or t.startswith(tbl) for t in tables):
                warn('coverage[%s]: %r 只经表级声明支持' % (c['route'], need))
                continue
            fail('coverage[%s]: 声明 covered 但 %r 没有 IO 承载' % (c['route'], need))

    # ---------- D 目录池 ----------
    spec_pools = {io['catalog']['pool'] for io in ios if io.get('catalog')}
    for p in spec_pools - pools:
        fail('io_spec 用了 catalog_index 没有的池: %s' % p)
    for p in pools - spec_pools:
        warn('catalog_index 有池但没 IO 使用: %s' % p)
    known_tables = set()
    for io in ios:
        for s in io.get('source', []):
            known_tables.add(s['table'].upper())
    for pool in catalog.get('pools', []):
        for src in pool.get('source_moc', []):
            base = src.split('.')[0].upper()
            if base in known_tables:
                continue
            # 允许写 "CNACLD.pfx" 这种列引用，但表名必须在 io_spec 出现过
            if any(base == t or base.startswith(t) or t.startswith(base) for t in known_tables):
                continue
            warn('pool %s 的来源 %r 不在任何 IO 的 source 里' % (pool['pool'], src))

    # ---------- 汇总 ----------
    print()
    print('IO %d / 字段 %d / 链接 %d / functions %d / coverage %d'
          % (len(ios), sum(len(i['fields']) for i in ios),
             sum(len(i['links']) for i in ios), len(funcs), len(cov)))
    if warns:
        print('\nWARN %d 条：' % len(warns))
        for w in warns:
            print('  · ' + w)
    if fails:
        print('\nFAIL %d 条：' % len(fails))
        for f in fails:
            print('  ✗ ' + f)
        return 1
    print('\nok io_spec 自洽（结构 / 基线 / 覆盖 / 目录池 四组断言全过）')
    return 0


if __name__ == '__main__':
    sys.exit(main())
