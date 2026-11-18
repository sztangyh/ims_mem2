# -*- coding: utf-8 -*-
"""MGW 导入验收：本仓的逐表 dump vs `ims_core` 的 `mgw_library.db`（金标准）。

判据（全部可机判）：
  ① **每张 dedicated 表的行数相等**（金标准 `mgw_<moc>_cur` vs 本仓）；
  ② **逐行逐列相等**：按自然键拼成 `k=v<US>…<TAB>col=val<US>…`，两侧集合**完全相等**；
  ③ **位图参数**在两侧都是"整数位掩码的十进制串"（`apply_bitmap` 口径）；
  ④ single 层（金标准没有 `_cur` 表）单独列出，不参与 ①②。

用法：
    python tools/check_mgw.py                # 自动跑 mgw_dump 再比
    python tools/check_mgw.py --dump X.tsv   # 用已有 dump
退出码：0 全过 / 1 有差异 / 4 缺输入（金标准库 / MGW 日志）
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ORACLE = r'C:\dev\ims_ws\ims_core\temp\mgw_library.db'
MGW_DATA = r'C:\dev\ims_ws\data\mgw'
BATCH_DIRS = ['202304', '20230919']       # 金标准建库时用的就是这两批（见 backup_batch 20 行）
US = '\x1f'

#: **已知且被解释的差异**：`moc → 允许的"金标准独有"行数`。
#:
#: `OFI` 的 `SET OFI: NAME=…;` 行**不带 INDEX**（键只在 `ADD OFI: INDEX=1,…` 里）。
#: ims_core 的 `merge_rows` 用 `r.get(k).unwrap_or_default()` → 给它造了个 `INDEX=""` 的行；
#: ims_mem2 的引擎把"键缺席"计为 `key_missing` 并**丢行**（table.rs 的既有语义，AGCF 侧同样规则）。
#: 这是两边的**设计差异**，不是移植错误 —— 所以这里显式计数，多一行就红。
ALLOW_ORACLE_ONLY = {'OFI': 2}


def oracle_lines():
    """从金标准库拼出与本仓 dump 同格式的行：`{MOC: {line, ...}}`。"""
    import sqlite3
    if not os.path.exists(ORACLE):
        print('x 缺金标准库：%s' % ORACLE, file=sys.stderr)
        return None
    if not os.path.exists(os.path.join(MGW_DATA, 'moc_key.json')):
        print('x 缺 MGW 元数据：%s' % MGW_DATA, file=sys.stderr)
        return None
    ky = json.load(open(os.path.join(MGW_DATA, 'moc_key.json'), encoding='utf-8'))
    sc = json.load(open(os.path.join(MGW_DATA, 'moc_schema.json'), encoding='utf-8'))
    keys_map = ky['keys']
    c = sqlite3.connect('file:%s?mode=ro' % ORACLE.replace('\\', '/'), uri=True)
    ne_short = {r[0]: r[1] for r in c.execute('SELECT ne_id, short FROM mgw_ne')}
    tabs = [r[0] for r in c.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'mgw\\_%\\_cur' ESCAPE '\\'")]
    out = {}
    for t in tabs:
        moc = t[len('mgw_'):-len('_cur')].upper()
        if moc not in sc['mocs']:
            continue
        keys = keys_map.get(moc, [])
        cols_all = sc['mocs'][moc]['cols']
        valcols = [x for x in cols_all if x not in keys]
        actual = {r[1] for r in c.execute('PRAGMA table_info("%s")' % t)}
        sel = ['ne_id'] + [x for x in keys if x in actual] + [x for x in valcols if x in actual]
        q = 'SELECT %s FROM "%s"' % (','.join('"%s"' % x for x in sel), t)
        lines = set()
        for r in c.execute(q):
            nid = r[0]
            vals = dict(zip(sel[1:], r[1:]))
            ks = [('%s=%s' % (k, vals.get(k) or '')) for k in keys if k in vals]
            cs = sorted('%s=%s' % (k, vals[k]) for k in valcols if k in vals and vals[k] is not None)
            lines.add('%s\t%s\t%s\t%s' % (moc, ne_short.get(nid, str(nid)), US.join(ks), US.join(cs)))
        out[moc] = lines
    c.close()
    return out


def run_dump(extra_dirs=()):
    exe = os.path.join(ROOT, 'target', 'release', 'examples', 'mgw_dump.exe')
    if not os.path.exists(exe):
        print('x 缺 %s（先 cargo build --release --example mgw_dump）' % exe, file=sys.stderr)
        return None
    dirs = [os.path.join(MGW_DATA, d) for d in BATCH_DIRS] + list(extra_dirs)
    for d in dirs:
        if not os.path.exists(d):
            print('x 缺 MGW 日志目录：%s' % d, file=sys.stderr)
            return None
    p = subprocess.run([exe] + dirs, capture_output=True, cwd=ROOT)
    if p.returncode != 0:
        print('x mgw_dump 失败（%d）：%s' % (p.returncode, p.stderr.decode('utf-8', 'replace')[-300:]),
              file=sys.stderr)
        return None
    return p.stdout.decode('utf-8', 'replace').splitlines()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--dump', help='用已有 dump 文件（不跑 mgw_dump）')
    a = ap.parse_args()

    ora = oracle_lines()
    if ora is None:
        return 4
    if a.dump:
        raw = open(a.dump, encoding='utf-8').read().splitlines()
    else:
        raw = run_dump()
        if raw is None:
            return 4

    mine = {}
    for ln in raw:
        if not ln:
            continue
        moc = ln.split('\t', 1)[0]
        mine.setdefault(moc, set()).add(ln)

    print('金标准 %d 张 mgw_*_cur 表 / 本仓 %d 张表' % (len(ora), len(mine)))
    bad = 0
    tot_o = tot_m = 0
    print('%-14s %8s %8s %8s' % ('MOC', '金标准', '本仓', '差'))
    print('-' * 44)
    for moc in sorted(ora):
        o = ora[moc]
        m = mine.get(moc, set())
        tot_o += len(o)
        tot_m += len(m)
        d = len(m) - len(o)
        flag = '' if not d else '  ←'
        if d:
            bad += 1
        print('%-14s %8d %8d %8d%s' % (moc, len(o), len(m), d, flag))
    print('-' * 44)
    print('%-14s %8d %8d %8d' % ('合计', tot_o, tot_m, tot_m - tot_o))

    extra = sorted(set(mine) - set(ora))
    if extra:
        print('\n本仓多出的表（%d 张，金标准没有对应 `_cur`）：%s' % (len(extra), ', '.join(extra)))

    # 逐行差异样例
    for moc in sorted(ora):
        o, m = ora[moc], mine.get(moc, set())
        if o == m:
            continue
        only_o, only_m = sorted(o - m), sorted(m - o)
        print('\n[%s] 金标准独有 %d / 本仓独有 %d' % (moc, len(only_o), len(only_m)))
        for x in only_o[:2]:
            print('  金标准: %s' % x.replace(US, ' | ')[:170])
        for x in only_m[:2]:
            print('  本仓  : %s' % x.replace(US, ' | ')[:170])

    # 白名单：只允许"金标准独有"，且必须与 ALLOW 完全一致（多一行/多一张表都红）
    allow_note = []
    unexplained = 0
    for moc in sorted(ora):
        o, m = ora[moc], mine.get(moc, set())
        if o == m:
            continue
        only_o, only_m = o - m, m - o
        allow = ALLOW_ORACLE_ONLY.get(moc, 0)
        if allow and not only_m and len(only_o) == allow:
            allow_note.append('%s: 金标准独有 %d 行（空键行，见 ALLOW_ORACLE_ONLY）' % (moc, allow))
            continue
        unexplained += 1
    if allow_note:
        print('\n已知差异（已解释，计入白名单）：')
        for x in allow_note:
            print('  - ' + x)
    if unexplained == 0 and bad <= len(allow_note):
        allowed = sum(ALLOW_ORACLE_ONLY.values())
        print('\nok MGW：%d 张 dedicated 表逐行逐列零差异（%d 行）；'
              '另有 %d 行属已解释的空键差异' % (len(ora) - len(allow_note), tot_o - allowed, allowed))
        return 0
    print('\nx MGW：%d 张表有未解释的差异，合计差 %d' % (unexplained, tot_m - tot_o))
    return 1


if __name__ == '__main__':
    sys.exit(main())
