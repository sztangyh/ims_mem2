# -*- coding: utf-8 -*-
"""`hist_diff` 自校验：old→new 链是否自洽。

三条检查（都是**跨来源**的，不是重算同一段代码）：
1. **连续**：同一 (ne, moc, row, col) 相邻两条版本，`valid_to` == 下一条的 `valid_from`；
2. **接续**：上一条的 `new` == 下一条的 `old`；
3. **落地**：每列最后一条的 `new` == `cur_<MOC>` 里该 (ne, row, col) 的当前值。

用法：python tools/check_hist_diff.py
退出码 0 = 全部通过。
"""
import os
import sqlite3
import sys

def _pick_db():
    """命令行给了就用它；否则挑 temp 下最新的 proj_hist*.db（export_hist 可能在换名）。"""
    import glob
    import sys
    if len(sys.argv) > 1:
        return sys.argv[1]
    cands = glob.glob(r'C:\dev\tryit\ims_mem2\temp\proj_hist*.db')
    if not cands:
        return r'C:\dev\tryit\ims_mem2\temp\proj_hist.db'
    return max(cands, key=os.path.getmtime)


DB = _pick_db()


def main():
    db = sqlite3.connect(DB)
    db.execute('PRAGMA temp_store=MEMORY')
    tables = {r[0] for r in db.execute("select name from sqlite_master where type='table'")}
    if 'hist_diff' not in tables:
        print('没有 hist_diff（先跑 export_hist 例子）', file=sys.stderr)
        return 2
    mocs = [r[0] for r in db.execute('select distinct moc from hist_diff order by moc')]
    print(f'hist_diff：{len(mocs)} 个 MOC')

    bad_cont = 0
    bad_chain = 0
    bad_land = 0
    checked = 0
    shown = 0
    for moc in mocs:
        cur_tbl = f'cur_{moc}'
        has_cur = cur_tbl in tables
        # 该 MOC 的当前值：只为 hist_diff 里出现过的 (ne,row) 取
        needed = {}
        for ne, row, col in db.execute(
                'select distinct ne, row, col from hist_diff where moc = ?', (moc,)):
            needed.setdefault(ne, {}).setdefault(row, set()).add(col)
        cur = {}
        if has_cur:
            cols = [r[1] for r in db.execute(f'PRAGMA table_info("{cur_tbl}")')]
            for ne, rows in needed.items():
                colset = set()
                for s in rows.values():
                    colset |= s
                sel = ','.join(f'"{c}"' for c in cols if c in colset)
                if not sel:
                    continue
                for r in db.execute(
                        f'select "row",{sel} from "{cur_tbl}" where "ne" = ?', (ne,)):
                    cur[(ne, r[0])] = dict(zip([c for c in cols if c in colset], r[1:]))

        q = f'''
        select ne, row, col, old, new, valid_from, valid_to,
               lead(old) over w as next_old,
               lead(valid_from) over w as next_vf
        from hist_diff where moc = ? window w as (partition by ne, row, col order by valid_from)
        '''
        for ne, row, col, old, new, vf, vt, next_old, next_vf in db.execute(q, (moc,)):
            checked += 1
            if next_vf is not None and vt != next_vf:
                bad_cont += 1
                if shown < 5:
                    print(f'  不连续 {moc} 行{row} 列{col}: valid_to {vt} != 下一条 {next_vf}')
                    shown += 1
            if next_old is not None and new != next_old:
                bad_chain += 1
                if shown < 5:
                    print(f'  断链 {moc} 行{row} 列{col}: new {new!r} != 下一条 old {next_old!r}')
                    shown += 1
            if next_old is None and has_cur:
                # 最后一版：new 必须等于当前值
                got = cur.get((ne, row))
                if got is None:
                    continue
                want = got.get(col)
                want = None if want is None else str(want)
                if new != want:
                    bad_land += 1
                    if shown < 5:
                        print(f'  落地不符 {moc} 行{row} 列{col}: new {new!r} != cur {want!r}')
                        shown += 1
    print(f'检查 {checked} 条记录：不连续 {bad_cont} / 断链 {bad_chain} / 落地不符 {bad_land}')
    return 0 if bad_cont == 0 and bad_chain == 0 and bad_land == 0 else 1


if __name__ == '__main__':
    sys.exit(main())
