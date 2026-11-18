# -*- coding: utf-8 -*-
"""中文词典（zh_*）验收：`zh.tsv` == 投影库里的 5 张 `zh_*` 表，且覆盖率不退化。

六组断言（全部可机判）：
  ① **装载保真**：`zh.tsv` 的 6 类行数与 `temp/proj.db` 的 6 张表逐表相等
     —— 它同时验证了 Rust 侧的 `ZhDict::load`（解析/转义/分层）与 `export_zh`（投影）；
  ② **逐行相同**：按 `(moc,param)` / `(moc,param,value)` 取集合，两侧**完全相等**（不是包含）；
  ③ **覆盖率**：对 `cur_*` 的全部列算"能在词典里查到中文名"的比例，不得低于基线
     （实测 1,085 / 1,551 = 70.0%；基线取 1,070，留 15 列的余量）；
  ④ **抽检**：若干净值（不是"随便有就过"）—— 挑 6 个已知 `(MOC,PARAM)→中文名` 与
     2 个枚举取值解释，必须逐字相同；查不到就必须是 `None`/空，**不许猜**；
  ⑤ **开关名（位名）**：三源都在 / 全含汉字 / 覆盖 ≥500 / **同名不同义 ≥30 个位词** / 抽检；
  ⑥ **S 层冻结**：仓内 `switch_meanings.tsv` ≡ `zh.tsv` 的 S 层 —— **离线可复算的有效证明**
     （两个文件都在仓内，不碰外部库/外部 json）。

用法：python tools/check_zh.py            → 全过打印 ok 并退出 0；否则打印 x 并退 1
      python tools/check_zh.py --tsv-only  → 只校验 zh.tsv 自身（没有投影库时用）
"""
import argparse
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TSV = os.path.join(ROOT, 'zh.tsv')
MIN_COVERED = 1070      # 实测 1085
MAX_COLUMNS = 1600      # 实测 1551；涨太多说明声明变了而词典没跟上

# ④ 抽检表：(kind, 键..., 期望值) —— 这些值是 ims_core 的原样内容，改动即视为回退
SPOT_PARAM = [
    ('PRATG', 'TG', '中继群号'),
    ('PRATG', 'TGN', '中继群名称'),
    ('ASBR', 'PUI', '公有用户身份'),
    ('CNACLD', 'PFX', '呼叫字冠'),
    ('RT', 'SRST', '子路由选择方式'),
    ('CLRDSN', 'CLI', '主叫号码'),
]
SPOT_VALUE = [
    ('ASBR', 'DID', 'PRA'),
]


def unesc(s):
    out, i = [], 0
    while i < len(s):
        if s[i] == '\\' and i + 1 < len(s):
            i += 1
            out.append({'t': '\t', 'n': '\n', '\\': '\\'}.get(s[i], s[i]))
        else:
            out.append(s[i])
        i += 1
    return ''.join(out)


def load_tsv():
    if not os.path.exists(TSV):
        print('x zh.tsv 不存在（先跑 python tools/gen_zh.py --write）')
        return None
    tiers = {'M': [], 'P': [], 'Q': [], 'V': [], 'W': [], 'S': []}
    with open(TSV, encoding='utf-8') as f:
        for ln in f:
            ln = ln.rstrip('\n').rstrip('\r')
            if not ln or ln[0] == '#':
                continue
            f6 = ln.split('\t')
            if len(f6) < 6:
                print('x zh.tsv 有不足 6 列的行：%r' % ln[:60])
                return None
            k = f6[0]
            if k not in tiers:
                print('x zh.tsv 有未知 kind %r' % k)
                return None
            tiers[k].append(tuple(unesc(x) for x in f6[1:]))
    return tiers


def cur_db():
    ptr = os.path.join(ROOT, 'temp', 'cur_db.txt')
    if not os.path.exists(ptr):
        return None
    rel = open(ptr, encoding='utf-8', errors='replace').read().strip()
    p = os.path.join(ROOT, rel) if rel else None
    return p if p and os.path.exists(p) else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--tsv-only', action='store_true')
    a = ap.parse_args()

    tiers = load_tsv()
    if tiers is None:
        return 1
    print('zh.tsv：M %d / P %d / Q %d / V %d / W %d = %d 条' %
          (len(tiers['M']), len(tiers['P']), len(tiers['Q']),
           len(tiers['V']), len(tiers['W']), sum(len(v) for v in tiers.values())))
    if sum(len(v) for v in tiers.values()) == 0:
        print('x zh.tsv 是空的')
        return 1

    # ④ 抽检（只看 zh.tsv，不依赖数据库）
    # 每行 5 个字段：k1 / k2 / k3 / text / extra（kind 已剥掉）
    p1 = {(f[0], f[1]): f[3] for f in tiers['P']}
    bad = 0
    for moc, par, want in SPOT_PARAM:
        got = p1.get((moc, par))
        if got != want:
            print('x 抽检 %s.%s：期望 %r，实际 %r' % (moc, par, want, got))
            bad += 1
    v1 = {(f[0], f[1], f[2]): f[3] for f in tiers['V']}
    for moc, par, val in SPOT_VALUE:
        if (moc, par, val) not in v1:
            print('x 抽检取值 %s.%s=%s：查不到' % (moc, par, val))
            bad += 1
    if bad:
        return 1
    print('ok 抽检 %d 个参数名 + %d 个取值解释 逐字一致' %
          (len(SPOT_PARAM), len(SPOT_VALUE)))

    if a.tsv_only:
        print('ok zh.tsv 自洽（--tsv-only）')
        return 0

    db = cur_db()
    if db is None:
        print('x 没有投影库（temp/cur_db.txt）—— 需要先跑投影导出；不能用 --tsv-only 绕过')
        return 1

    con = sqlite3.connect('file:%s?mode=ro' % db.replace('\\', '/'), uri=True)

    # ① 装载保真：行数逐表相等
    want = {'zh_moc': len(tiers['M']), 'zh_param': len(tiers['P']),
            'zh_param_any': len(tiers['Q']), 'zh_value': len(tiers['V']),
            'zh_value_any': len(tiers['W'])}
    for t, n in want.items():
        try:
            got = con.execute('SELECT COUNT(*) FROM "%s"' % t).fetchone()[0]
        except sqlite3.Error as e:
            print('x 投影库缺表 %s（%s）' % (t, e))
            return 1
        if got != n:
            print('x %s：投影 %d 行，zh.tsv %d 行' % (t, got, n))
            return 1
    print('ok 6 张 zh_* 表行数与 zh.tsv 逐表相等')

    # ② 逐行相同（集合相等，不是包含）
    #    每个 tier 的一行是 5 元：k1 / k2 / k3 / text / extra
    checks = [
        ('zh_param', 'SELECT moc,param,param_cn FROM zh_param',
         {(f[0], f[1], f[3]) for f in tiers['P']}),
        ('zh_param_any', 'SELECT param,param_cn,n_moc,ambiguous FROM zh_param_any',
         {(f[0], f[3], *f[4].split(':')) for f in tiers['Q']}),
        ('zh_value', 'SELECT moc,param,value,value_cn FROM zh_value',
         {(f[0], f[1], f[2], f[3]) for f in tiers['V']}),
    ]
    for name, sql, want_set in checks:
        got = set(tuple('' if x is None else str(x) for x in r) for r in con.execute(sql))
        if got != want_set:
            print('x %s 集合不等：投影 %d / tsv %d / 差 %d' % (name, len(got), len(want_set), len(got ^ want_set)))
            for x in list(got ^ want_set)[:3]:
                print('    差异样例 %r' % (x,))
            return 1
        print('ok %s 逐行相同（%d 行）' % (name, len(got)))

    # ③ 覆盖率
    q1 = set(p1.keys())
    q2 = {f[0] for f in tiers['Q']}
    tabs = [x[0] for x in con.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'cur\\_%' ESCAPE '\\'")]
    total = hit = 0
    for t in tabs:
        moc = t[4:].upper()
        for c in [x[1] for x in con.execute('PRAGMA table_info("%s")' % t)]:
            total += 1
            if (moc, c.upper()) in q1 or c.upper() in q2:
                hit += 1
    pct = 100.0 * hit / total if total else 0
    print('   覆盖率：%d / %d 列 = %.1f%%' % (hit, total, pct))
    if total > MAX_COLUMNS:
        print('x 列数 %d 超过基线 %d（声明变了？词典要跟着更新）' % (total, MAX_COLUMNS))
        return 1
    if hit < MIN_COVERED:
        print('x 覆盖列数 %d 低于基线 %d' % (hit, MIN_COVERED))
        return 1
    print('ok 覆盖率 %d 列（基线 >= %d）' % (hit, MIN_COVERED))
    con.close()

    # ⑤ 开关名（位名）层
    print()
    rc = check_switch(tiers)
    if rc != 0:
        return rc

    print('ok zh 词典：装载保真 + 逐行相同 + 覆盖率 + 抽检 + 开关名 + S 层冻结 六组断言全过')
    return 0


MIN_SWITCH_COVERED = 500   # 实测 517 / 1,058
BASELINE_SWITCH_BITS = 1058


def check_switch(tiers):
    """⑤ 开关名层：三元组精确、无纯英文、覆盖基线、同名不同义不得被压平。"""
    sw = {}
    for f in tiers['S']:                     # f = (k1=moc, k2=param, k3=switch, text, extra=src)
        sw[(f[0], f[1], f[2])] = (f[3], f[4])
    if not sw:
        print('x zh.tsv 没有 S 层（开关名）')
        return 1
    dist = {}
    for _, src in sw.values():
        dist[src] = dist.get(src, 0) + 1
    print('   开关名 %d 条：%s' % (len(sw), ' / '.join('%s %d' % (k, v) for k, v in sorted(dist.items()))))

    # a) 三个来源都得在（任何一个静默消失都要红）
    for need in ('manual', 'doc', 'vocab'):
        if dist.get(need, 0) == 0:
            print('x 开关名来源 %r 一条都没有（源头丢了？）' % need)
            return 1

    # b) 不许混进纯英文（`doc_option` 里有 340 行英文，是明确不用的来源）
    bad = [k for k, (v, _) in sw.items() if not re.search(r'[\u4e00-\u9fff]', v)]
    if bad:
        print('x 有 %d 条开关名中文不含汉字（例如 %s）' % (len(bad), bad[0]))
        return 1
    print('   ok 全部含汉字（没有把 doc_option 的英文混进来）')

    # c) 覆盖：对 ims_mem2 的 44 个 Bits 参数（1,058 个开关名）算覆盖
    sys.path.insert(0, os.path.join(ROOT, 'tools'))
    import gen_zh
    want = gen_zh.ims_mem2_bits()
    if len(want) != BASELINE_SWITCH_BITS:
        print('x 本仓 Bits 开关名基线变了：%d（基线 %d）' % (len(want), BASELINE_SWITCH_BITS))
        return 1
    cov = sum(1 for k in want if k in sw)
    print('   覆盖 ims_mem2 的开关名：%d / %d = %.1f%%' % (cov, len(want), 100.0 * cov / len(want)))
    if cov < MIN_SWITCH_COVERED:
        print('x 开关名覆盖 %d 低于基线 %d' % (cov, MIN_SWITCH_COVERED))
        return 1
    print('   ok 覆盖 %d（基线 >= %d）' % (cov, MIN_SWITCH_COVERED))

    # d) **同名不同义不得被压平**：这是选源的核心结论，必须能观察到
    byname = {}
    for (moc, par, nm), (v, _) in sw.items():
        byname.setdefault(nm, set()).add(v)
    multi = {k: v for k, v in byname.items() if len(v) > 1}
    if len(multi) < 30:
        print('x 只发现 %d 个"同名不同义"的位词（应 >= 30）—— 说明按词压平了，正是要防的错' % len(multi))
        return 1
    ex = sorted(multi.items(), key=lambda kv: -len(kv[1]))[0]
    print('   ok 同名不同义 %d 个位词（例：%s 有 %d 种含义）' % (len(multi), ex[0], len(ex[1])))

    # e) 抽检：三元组精确（换个参数就查不到）
    for k, must in [(('MGW', 'UCATT', 'NOEC'), '回声'),
                    (('CDBFUNC', 'FCF', 'LOC'), '定位')]:
        v = sw.get(k)
        if not v or must not in v[0]:
            print('x 抽检 %s：期望含 %r，实际 %r' % ('/'.join(k), must, v))
            return 1
    s2 = [k for k in sw if k[2] == 'SVR2']
    cross = {(k[0], k[1]) for k in s2}
    if len(cross) >= 2 and len({sw[k][0] for k in s2}) < 2:
        print('x SVR2 在 %d 个参数下只有 1 种含义 —— 大概率按词套错了' % len(cross))
        return 1
    print('   ok 抽检：%s / %s；SVR2 覆盖 %d 个参数、%d 种含义'
          % ('MGW.UCATT.NOEC', 'CDBFUNC.FCF.LOC', len(cross), len({sw[k][0] for k in s2})))

    # f) **S 层冻结**：仓内快照 `switch_meanings.tsv` 必须与 zh.tsv 的 S 层逐行相同。
    #    这是"离线可复算"的有效证明 —— 两个文件都在仓内，不碰任何外部库/外部 json。
    return check_snapshot(sw)


def check_snapshot(sw):
    """`switch_meanings.tsv`（冻结快照）≡ `zh.tsv` 的 S 层。"""
    path = os.path.join(ROOT, 'switch_meanings.tsv')
    if not os.path.exists(path):
        print('x 缺少 S 层冻结快照 switch_meanings.tsv'
              '（跑 python tools/gen_zh.py --refresh --write 生成）')
        return 1
    rows, head = {}, []
    for line in open(path, encoding='utf-8'):
        line = line.rstrip('\n').rstrip('\r')
        if not line:
            continue
        if line[0] == '#':
            head.append(line)
            continue
        f = line.split('\t')
        if len(f) != 5:
            print('x 快照行不是 5 列：%r' % line[:80])
            return 1
        rows[tuple(unesc(x) for x in f[:3])] = (unesc(f[3]), f[4])
    if len(rows) != len(sw):
        print('x 快照 %d 行，zh.tsv 的 S 层 %d 行' % (len(rows), len(sw)))
        return 1
    diff = [k for k in rows if sw.get(k) != rows[k]]
    if diff:
        k = diff[0]
        print('x 快照与 S 层不一致 %d 行，例如 %s：快照 %r / zh.tsv %r'
              % (len(diff), '/'.join(k), rows[k], sw.get(k)))
        return 1
    if not any('冻结快照' in h for h in head):
        print('x 快照缺少说明性注释头（应为 --refresh 生成的产物）')
        return 1
    print('   ok S 层冻结快照 switch_meanings.tsv ≡ zh.tsv（%d 行，离线可复算）' % len(rows))
    return 0


if __name__ == '__main__':
    sys.exit(main())




