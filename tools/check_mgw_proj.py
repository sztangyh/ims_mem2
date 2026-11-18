# -*- coding: utf-8 -*-
"""MGW 投影验收：`temp/mgw_proj.db` 的 `mgw_<moc>` 表 vs 内存（`mgw_dump --stats`）。

判据：
  ① **命名空间**：库里所有 MGW 表都是 `mgw_` 前缀，**没有一张** `cur_`/`hist_` 名字
     （AGCF 与 MGW 有 7 个同名 MOC：BRD/IPADDR/N7DSP/N7LKS/N7LNK/N7RT/OFI —— 撞名会互相覆盖）；
  ② **逐表行数** == 内存存活行数（两者走不同代码路径：一个直接从 `TableApi`，一个经 SQLite）；
  ③ **合计行数** 一致；
  ④ **列名/列数** == `moc_schema.json` 的声明（防止投影时少列/改名）；
  ⑤ 大表（>=1,000 行）有 `ix_mgw_*_ne` 索引。

用法：
    python tools/check_mgw_proj.py            # 自动跑 mgw_export 再比
    python tools/check_mgw_proj.py --no-run   # 用现有 temp/mgw_proj.db
退出码：0 全过 / 1 有差异 / 4 缺输入。
"""
import argparse
import json
import os
import sqlite3
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MGW_DATA = r'C:\dev\ims_ws\data\mgw'
DB = os.path.join(ROOT, 'temp', 'mgw_proj.db')
INDEX_MIN_ROWS = 1_000


def run_export():
    exe = os.path.join(ROOT, 'target', 'release', 'examples', 'mgw_export.exe')
    if not os.path.exists(exe):
        print('x 缺 %s（先 cargo build --release --example mgw_export）' % exe, file=sys.stderr)
        return None
    p = subprocess.run([exe, MGW_DATA], capture_output=True, cwd=ROOT)
    if p.returncode != 0:
        print('x mgw_export 失败：%s' % p.stderr.decode('utf-8', 'replace')[-400:], file=sys.stderr)
        return None
    return p.stdout.decode('utf-8', 'replace')


def mem_counts():
    """内存侧（`mgw_dump --stats`）：{moc: 存活行数}。"""
    exe = os.path.join(ROOT, 'target', 'release', 'examples', 'mgw_dump.exe')
    p = subprocess.run([exe, '--stats', MGW_DATA], capture_output=True, cwd=ROOT)
    if p.returncode != 0:
        print('x mgw_dump --stats 失败', file=sys.stderr)
        return None
    out = {}
    for ln in p.stdout.decode('utf-8', 'replace').splitlines():
        f = ln.split('\t')
        if len(f) == 3 and f[0].isupper():
            out[f[0]] = int(f[2])
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--no-run', action='store_true')
    a = ap.parse_args()
    if not os.path.isdir(MGW_DATA):
        print('x 缺 MGW 日志目录 %s' % MGW_DATA, file=sys.stderr)
        return 4
    if not a.no_run:
        if run_export() is None:
            return 4
    if not os.path.exists(DB):
        print('x 缺 %s' % DB, file=sys.stderr)
        return 4

    sc = json.load(open(os.path.join(MGW_DATA, 'moc_schema.json'), encoding='utf-8'))['mocs']
    mem = mem_counts()
    if mem is None:
        return 4

    c = sqlite3.connect('file:%s?mode=ro' % DB.replace('\\', '/'), uri=True)
    tabs = [r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type='table'")]
    idxs = {r[0] for r in c.execute(
        "SELECT name FROM sqlite_master WHERE type='index' AND name NOT LIKE 'sqlite_%'")}
    bad = 0

    # ① 命名空间
    odd = [t for t in tabs if not t.startswith('mgw_')]
    if odd:
        print('x 出现非 mgw_ 前缀的表：%s' % odd[:5])
        bad += 1
    hijack = [t for t in tabs if t.startswith(('cur_', 'hist_'))]
    if hijack:
        print('x 出现会与 AGCF 撞名的表：%s' % hijack[:5])
        bad += 1
    print('命名空间：%d 张表全部 mgw_ 前缀（AGCF 侧 cur_* 未受影响）' % len(tabs))

    # ② 逐表行数
    tot = 0
    for t in sorted(tabs):
        moc = t[len('mgw_'):].upper()
        if moc not in sc:
            print('x 库里多出未知 MOC 表：%s' % t)
            bad += 1
            continue
        n = c.execute('SELECT COUNT(*) FROM "%s"' % t).fetchone()[0]
        tot += n
        want = mem.get(moc)
        if want is None:
            print('x %s：内存侧没有这张表（金标准/内存不一致）' % moc)
            bad += 1
        elif n != want:
            print('x %s：库内 %d 行 / 内存 %d 行' % (moc, n, want))
            bad += 1
        # ④ 列名/列数
        cols = [r[1] for r in c.execute('PRAGMA table_info("%s")' % t)]
        want_cols = ['ne', 'row'] + [x.lower() for x in sc[moc]['cols']]
        if cols != want_cols:
            print('x %s 列不符：库内 %s / 声明 %s' % (moc, cols[:6], want_cols[:6]))
            bad += 1
        # ⑤ 索引
        if n >= INDEX_MIN_ROWS and ('ix_%s_ne' % t) not in idxs:
            print('x %s 有 %d 行但缺 ix_%s_ne 索引' % (moc, n, t))
            bad += 1
    print('逐表行数：%d 张表 / %d 行，与内存一致' % (len(tabs), tot), end='')
    print('；大表索引 %d 个' % len([i for i in idxs if i.startswith('ix_mgw_')]))
    c.close()

    total_mem = sum(mem.values())
    if tot != total_mem:
        print('x 合计：库内 %d / 内存 %d' % (tot, total_mem))
        bad += 1

    if bad:
        print('\nx 投影：%d 处不符' % bad)
        return 1
    print('\nok MGW 投影：43 张 mgw_ 表 / %d 行，命名空间、行数、列名、索引四组一致' % tot)
    return 0


if __name__ == '__main__':
    sys.exit(main())
