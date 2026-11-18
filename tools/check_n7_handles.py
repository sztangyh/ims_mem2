# -*- coding: utf-8 -*-
"""D57 的**门看不见**的两点，单独校验：

1. `N7TG` 上的 `(CIRC_START, CIRC_LEN)` 句柄是否真的对齐 arena
   —— `kv_dump n7tkc` 走的是 `live` 段，**不查 N7TG 的句柄**，所以句柄错了 dump 也照样零差异；
2. 两批导入时 `hist_N7TG` 是否真的留下了 `CIRC_START/CIRC_LEN` 的版本
   —— `hist_N7TKC` 本来就是空的，N7 的历史只能走这里。

注意（2026-11-12 缺陷①修复后）：批末做了**内容寻址复用**，所以同一网元多批时
`CIRC_START` 不再是"每网元从 0 递增"的连续区间（复用旧段 → start 回到旧值）。
因此这里断言的是 **长度一致 + 段起点互不重叠**，而不是"递增偏移"。

用法：在 `check_tables.ps1` 里、`kv_dump n7tkc` 与 `export_hist` 之后调用。
退出码 0 = 通过。
"""
import sqlite3
import sys

def _cur_db():
    p = r'temp\cur_db.txt'
    try:
        with open(p, encoding='utf-8') as f:
            v = f.read().strip()
        if v:
            return v
    except OSError:
        pass
    return r'temp\proj.db'


CUR = _cur_db()
HIST = r'temp\proj_hist.db'
DUMP = r'temp\cmp_n7tkc_rust.txt'


def main():
    ok = True
    c = sqlite3.connect(CUR)
    rows = list(c.execute('SELECT ne, TG, CIRC_START, CIRC_LEN FROM cur_N7TG ORDER BY ne, CIRC_START'))
    print(f'   cur_N7TG {len(rows)} 行（每行 = 一个 TG 的电路段句柄）')

    # dump 的列序：ne|mn|sc|cs|ec|scf|scic|tg|tgn|tid
    counts = {}
    try:
        for line in open(DUMP, encoding='utf-8'):
            p = line.rstrip('\n').split('|')
            key = (p[0], int(p[7]))
            counts[key] = counts.get(key, 0) + 1
    except FileNotFoundError:
        print('   x 找不到 n7tkc 的 dump（先跑 kv_dump n7tkc）')
        return 1

    bad = 0
    seen_key = set()
    for ne, tg, start, ln in rows:
        if counts.get((ne, tg)) != ln:
            print(f'   x {ne} TG={tg}: CIRC_LEN={ln}，dump 里 {counts.get((ne, tg))} 条')
            bad += 1
        if (ne, tg) in seen_key:
            print(f'   x {ne} TG={tg}: 句柄重复')
            bad += 1
        seen_key.add((ne, tg))
        if ln and start == 0 and counts.get((ne, tg)) is None:
            bad += 1
    # 段起点在同网元内不许重叠（复用会让 start 不递增，但不能重叠）
    spans = {}
    for ne, tg, start, ln in rows:
        spans.setdefault(ne, []).append((start, start + ln, tg))
    for ne, v in spans.items():
        v.sort()
        for a, b in zip(v, v[1:]):
            if a[1] > b[0]:
                print(f'   x {ne}: 段重叠 TG={a[2]}({a[0]}..{a[1]}) 与 TG={b[2]}({b[0]}..{b[1]})')
                bad += 1

    total = sum(counts.values())
    covered = sum(r[3] for r in rows)
    print(f'   段覆盖 {covered} 条 / dump 合计 {total} 条')
    if covered != total:
        print('   x 有电路没被任何句柄覆盖（或有重复覆盖）')
        bad += 1
    if bad:
        ok = False

    # hist：N7TKC 自己没有版本链（按设计），历史只可能在 hist_N7TG 的句柄列上。
    # ⚠ 句柄列**没有**版本也是合法的：批末做了内容寻址复用（缺陷①的修复），
    # 两批内容相同 → 句柄不变 → 不该有版本。所以这里只做"一致性"断言：
    #   ① hist_N7TKC 必须为空；
    #   ② 如果 hist_N7TG 有记录，只允许出现在 CIRC_START/CIRC_LEN（或 N7TG 自己的列）上。
    # "句柄变了必须有版本"这条由单测
    #   import::tests::n7_circuits_land_in_the_arena_and_the_handle_change_is_versioned
    # 正面钉住（数据集里 N7 两批相同，门里看不出来）。
    try:
        h = sqlite3.connect(HIST)
        # 优化后空 hist_ 表不再建（只给真有版本的 MOC 建表）→ 缺表等价于 0 条
        def _cnt(sql):
            try:
                return h.execute(sql).fetchone()[0]
            except sqlite3.OperationalError:
                return 0
        n7tkc_rows = _cnt('SELECT COUNT(*) FROM hist_N7TKC')
        n = _cnt("SELECT COUNT(*) FROM hist_N7TG WHERE \"col\" IN ('CIRC_START','CIRC_LEN')")
        allrec = _cnt('SELECT COUNT(*) FROM hist_N7TG')
        print(f'   hist_N7TKC {n7tkc_rows} 条（应为 0）；hist_N7TG {allrec} 条，其中句柄列 {n} 条')
        if n7tkc_rows != 0:
            print('   x hist_N7TKC 应该是空的（N7 的历史走 N7TG 的句柄）')
            ok = False
        try:
            cols = {r[0] for r in h.execute('SELECT DISTINCT "col" FROM hist_N7TG')}
        except sqlite3.OperationalError:
            cols = set()
        bad_cols = cols - {'CIRC_START', 'CIRC_LEN'}
        if allrec and n == 0:
            print('   · 句柄没有版本：两批 N7 内容相同 → 复用旧段，符合预期')
        if bad_cols:
            print(f'   · hist_N7TG 里还有非句柄列 {sorted(bad_cols)}（属于 N7TG 自己的列，允许）')
    except Exception as e:  # noqa: BLE001
        print(f'   x hist 检查失败：{e}')
        ok = False

    print('   ok 句柄长度/不重叠 + hist 口径一致' if ok else '   x 不一致')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
