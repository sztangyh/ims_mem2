# -*- coding: utf-8 -*-
"""投影索引验收（P5a+）：热表必须有 `(ne)` 索引，且查询计划真的走索引。

背景（实测，`temp/proj.db` 156.5 MB / 943,104 行 / 149 张 `cur_*`）：

| 方案 | 建索引耗时 | 库体积 | `cur_ASBR WHERE ne=?` |
|---|---|---|---|
| 无索引（改动前） | — | 156.5 MB | **265.5 ms** |
| 只建行数 >= 1000 的 21 张 | +4.7 s | +13.3 MB | **15.4 ms** |

三组断言：
  ① **索引存在**：带索引的表数 == 行数 >= 1,000 的 `cur_*` 表数（与 `INDEX_MIN_ROWS` 同步）；
  ② **命名与归属**：每个索引名形如 `ix_cur_<MOC>_ne`，且它的表就是 `cur_<MOC>`；
  ③ **计划生效**：`EXPLAIN QUERY PLAN SELECT ... WHERE ne=?` 对最热的三张表必须出现 `SEARCH`
     （出现 `SCAN` 即视为索引没用上 —— 那比没建索引更糟，因为白占体积）。

用法：python tools/check_indexes.py [db 路径]   （缺省跟随 temp/cur_db.txt）
"""
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MIN_ROWS = 1000          # 必须与 crates/proj/src/lib.rs 的 INDEX_MIN_ROWS 一致
HOT = ['ASBR', 'CNACLD', 'SRTCOST']


def cur_db():
    ptr = os.path.join(ROOT, 'temp', 'cur_db.txt')
    if not os.path.exists(ptr):
        return None
    rel = open(ptr, encoding='utf-8', errors='replace').read().strip()
    p = os.path.join(ROOT, rel) if rel else None
    return p if p and os.path.exists(p) else None


def main():
    db = sys.argv[1] if len(sys.argv) > 1 else cur_db()
    if not db or not os.path.exists(db):
        print('x 找不到投影库（temp/cur_db.txt 或参数）')
        return 1
    con = sqlite3.connect('file:%s?mode=ro' % db.replace('\\', '/'), uri=True)

    tabs = [x[0] for x in con.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'cur\\_%' ESCAPE '\\'")]
    rows = {t: con.execute('SELECT COUNT(*) FROM "%s"' % t).fetchone()[0] for t in tabs}
    hot = sorted((t for t, n in rows.items() if n >= MIN_ROWS), key=lambda t: -rows[t])
    idx = [x[0] for x in con.execute(
        "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'ix\\_cur\\_%' ESCAPE '\\'")]
    print('   cur_* %d 张 / 索引 %d 个 / 门槛行数 %d（应建 %d 张）' %
          (len(tabs), len(idx), MIN_ROWS, len(hot)))

    # ① 数量
    if len(idx) != len(hot):
        print('   x 索引数 %d != 应建数 %d' % (len(idx), len(hot)))
        miss = set('ix_cur_%s_ne' % t[4:] for t in hot) - set(idx)
        print('     缺：%s' % (', '.join(sorted(miss)) or '（无）'))
        return 1

    # ② 命名与归属
    for name in idx:
        m = re.fullmatch(r'ix_cur_(\w+)_ne', name)
        if not m:
            print('   x 索引命名不合规：%s' % name)
            return 1
        tbl = 'cur_' + m.group(1)
        cols = [r[2] for r in con.execute('PRAGMA index_info("%s")' % name)]
        if cols != ['ne']:
            print('   x %s 的索引列是 %s，应为 ["ne"]' % (name, cols))
            return 1
        if tbl not in tabs:
            print('   x %s 指向不存在的表 %s' % (name, tbl))
            return 1
    print('   ok 索引数量 / 命名 / 列 全部合规')

    # ③ 计划生效（SCAN = 索引没被用上）
    for moc in HOT:
        t = 'cur_' + moc
        if t not in rows:
            print('   . %s 不存在，跳过计划检查' % t)
            continue
        q = 'SELECT COUNT(*) FROM "%s" WHERE ne=?' % t
        plan = ' | '.join(
            r[3] for r in con.execute('EXPLAIN QUERY PLAN ' + q, ('AGCF51',)))
        if 'SEARCH' not in plan:
            print('   x %s 的查询计划没有走索引：%s' % (t, plan))
            return 1
        print('   ok %-12s %s' % (t, plan))
    con.close()
    print('   ok 投影索引：数量 / 命名 / 查询计划 三组断言全过')
    return 0


if __name__ == '__main__':
    sys.exit(main())
