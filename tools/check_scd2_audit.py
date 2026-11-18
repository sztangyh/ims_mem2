# -*- coding: utf-8 -*-
"""SCD2 全表审计：用**原始 MML**当独立真值，逐 MOC 核对引擎的双批结果。

口径：
- 2023 批 = A51（AGCF51）/ A52（AGCF52）；2026 批 = ALLME_033056（AGCF51）/ ALLME_0330361（AGCF52）；
- 每个 MOC 的键 = 声明里的 `key [...]`（列名 → 参数名）；数值列按整数比，其余按原文比；
- 期望：`cur_<MOC>` 的存活键 == 2026 批的键集合；被删键 == 2023\2026；
- 输出只打印**不一致**的表（另外给一张"检查了多少表"的汇总）。

用法：python tools/check_scd2_audit.py [proj_hist.db]
"""
import collections
import glob
import io
import os
import re
import sqlite3
import sys

ROOT = r'C:\dev\tryit\ims_mem2'
FILES = {
    'AGCF51': ('C:\\dev\\ims_ws\\data\\agcf\\20230919\\A51_20230919.txt',
               r'C:\dev\tryit\agcf_data\ALLME_20260921033056.txt'),
    'AGCF52': ('C:\\dev\\ims_ws\\data\\agcf\\20230919\\A52_20230919.txt',
               r'C:\dev\tryit\agcf_data\ALLME_202609210330361.txt'),
}

MML = re.compile(rb'^(ADD|MOD|SET|RMV)\s+([A-Z0-9]+):')
ME_LINE = re.compile(rb'ADD ME:')
USE_ME = re.compile(rb'USE ME:MEID=(\d+)')
DECL = re.compile(r'mml_table!\s*\{\s*\n\s*([A-Z0-9_]+)\s*,\s*\w+\s*,\s*\n(.*?)\n\}', re.S)
FIELD = re.compile(r'^\s{4}(?:pub )?(?:r#)?([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*'
                   r'([A-Za-z][A-Za-z0-9_]*)\s*(?:\([^)]*\))?\s*(?:\[(\d+)\])?\s*,', re.M)
KEY_LINE = re.compile(r'^\s*key\s*\[([^\]]*)\]', re.M)

NUM = {'U8', 'U16', 'U32'}

# 这几个 MOC 在引擎里**不是逐行表**，所以没有 hist_row（也不该有）：
#   SRVRGT → 整表并入 ASBR（P7，cur_SRVRGT 是视图）
#   N7TKC  → 稀疏 Circuit arena（D57，按 TG 聚成句柄，没有"行"的生命周期）
NOT_ROW_TABLES = {'SRVRGT', 'N7TKC'}
BOOL = {'Bool'}


def declarations():
    """{MOC: (key_params, {param: kind})}；kind ∈ n/b/t/e/w；参数组展开成 NAME1..N。"""
    srcs = [os.path.join(ROOT, 'crates', 'core', 'src', 'schema_gen_data.rs')]
    d = os.path.join(ROOT, 'crates', 'core', 'src', 'schema')
    srcs += [os.path.join(d, f) for f in sorted(os.listdir(d)) if f.endswith('.rs')]
    out = {}
    for path in srcs:
        if not os.path.exists(path):
            continue
        text = io.open(path, encoding='utf-8').read()
        for m in DECL.finditer(text):
            moc, body = m.group(1), m.group(2)
            kinds, group_cols = {}, {}
            for fm in FIELD.finditer(body):
                name, ty, width = fm.group(1).upper(), fm.group(2), fm.group(3)
                kind = ('n' if ty in NUM else 'b' if ty in BOOL else 'e' if ty == 'Enum'
                        else 'w' if ty in ('Ip', 'Eid', 'Bits', 'Pair', 'Spc') else 't')
                if width:
                    types = {0: ('U16', 'n'), 1: ('U8', 'n')}
                    for i in range(1, int(width) + 1):
                        kinds[f'{name}{i}'] = kind
                else:
                    kinds[name] = kind
            km = KEY_LINE.search(body)
            keys = [k.strip().upper() for k in km.group(1).split(',')] if km else []
            out[moc] = (keys, kinds, group_cols)
    return out


def split_params(body):
    out, i, inq, start = [], 0, False, 0
    while i < len(body):
        c = body[i]
        if c == 0x22:
            inq = not inq
        elif c == 0x2C and not inq:
            seg = body[start:i]
            eq = seg.find(b'=')
            if eq > 0:
                out.append((seg[:eq].strip().upper().decode(), seg[eq + 1:].strip()))
            start = i + 1
        i += 1
    seg = body[start:]
    eq = seg.find(b'=')
    if eq > 0:
        out.append((seg[:eq].strip().upper().decode(), seg[eq + 1:].strip()))
    return out


def unq(v):
    return v[1:-1] if len(v) >= 2 and v[0] == 0x22 and v[-1] == 0x22 else v


def scan_keys(path, decls):
    """{moc: set(规范性键字符串)}；只取 ADD/SET/MOD（RMV 单独计数）。"""
    data = open(path, 'rb').read()
    meid2name, cur = {}, None
    out = collections.defaultdict(set)
    for line in data.split(b'\n'):
        line = line.rstrip(b'\r; \t')
        if not line:
            continue
        m = USE_ME.search(line)
        if m:
            cur = meid2name.get(int(m.group(1)))
            continue
        if ME_LINE.search(line):
            meid = name = None
            for (k, v) in split_params(line[line.find(b':') + 1:]):
                if k == 'MEID':
                    try:
                        meid = int(unq(v))
                    except ValueError:
                        pass
                elif k == 'MENAME':
                    name = unq(v).decode('gbk', 'replace')
            if meid is not None and name:
                meid2name[meid] = name
            continue
        m = MML.match(line)
        if not m or cur is None:
            continue
        op = m.group(1).decode()
        if op == 'RMV':
            continue
        moc = m.group(2).decode()
        if moc not in decls:
            continue
        keys, kinds, _ = decls[moc]
        if not keys:
            continue
        vals = {}
        for (k, v) in split_params(line[m.end():]):
            vals[k] = unq(v)
        parts = []
        ok = True
        for k in keys:
            if k not in vals:
                ok = False
                break
            raw = vals[k]
            kind = kinds.get(k, 't')
            if kind in ('n', 'b'):
                try:
                    parts.append(str(int(raw)))
                except ValueError:
                    ok = False
                    break
            else:
                try:
                    parts.append(raw.decode('gbk'))
                except UnicodeDecodeError:
                    parts.append(raw.decode('gbk', 'replace'))
        if ok:
            out[moc].add('\x1f'.join(parts))
    return out


def db_keys(con, ne, moc, keys, kinds):
    """从 cur_<MOC> 取**指定网元**的键（按同样的规范拼串）。"""
    t = 'cur_' + moc
    # 只认真表：cur_SRVRGT / cur_N7TKC 等是**视图**（并入/稀疏化），语义与逐行表不同
    r = con.execute("select type from sqlite_master where name = ?", (t,)).fetchone()
    if not r or r[0] != 'table':
        return None
    cols = {r[1].upper(): r[1] for r in con.execute(f'PRAGMA table_info("{t}")')}
    sel = ','.join(f'"{cols[k]}"' for k in keys if k in cols)
    if sel.count(',') + 1 != len(keys):
        return None
    out = set()
    for row in con.execute(f'select {sel} from "{t}" where "ne" = ?', (ne,)):
        parts = []
        for k, v in zip(keys, row):
            if v is None:
                parts.append('')
            elif kinds.get(k) in ('n', 'b'):
                parts.append(str(int(v)))
            else:
                parts.append(str(v))
        out.add('\x1f'.join(parts))
    return out


def main():
    db = sys.argv[1] if len(sys.argv) > 1 else max(
        glob.glob(os.path.join(ROOT, 'temp', 'proj_hist*.db')), key=os.path.getmtime)
    print('审计库:', os.path.basename(db))
    decls = declarations()
    con = sqlite3.connect('file:' + db.replace('\\', '/') + '?mode=ro', uri=True)

    has_hist_row = bool(con.execute(
        "select 1 from sqlite_master where name='hist_row'").fetchone())
    total = checked = 0
    bad = []
    bad_row = []
    for ne, (f2023, f2026) in FILES.items():
        old = scan_keys(f2023, decls)
        new = scan_keys(f2026, decls)
        for moc in sorted(set(old) & set(new)):
            keys, kinds, _ = decls[moc]
            got = db_keys(con, ne, moc, keys, kinds)
            if got is None:
                continue
            checked += 1
            total += 1
            want = new[moc]
            extra = got - want          # 僵尸：cur 里有、2026 批没有
            missing = want - got        # 丢行：2026 有、cur 没有
            if extra or missing:
                bad.append((ne, moc, len(got), len(want), len(extra), len(missing)))
            if has_hist_row and moc not in NOT_ROW_TABLES:
                alive_n, dead_n = con.execute(
                    'select sum(case when dead_at is null then 1 else 0 end), '
                    'sum(case when dead_at is not null then 1 else 0 end) '
                    'from hist_row where ne = ? and moc = ?', (ne, moc)).fetchone()
                alive_n, dead_n = alive_n or 0, dead_n or 0
                # hist_row 只记"生命周期非平凡"的行：
                #   首批创建且仍活着 → 不写（= cur 里的普通行）
                #   所以活着的记录数应 == 只在 2026 出现的键数（= 第二批新建的行）
                want_alive = len(want - old[moc])
                want_dead = len(old[moc] - want)
                if alive_n != want_alive or dead_n != want_dead:
                    bad_row.append((ne, moc, alive_n, want_alive, dead_n, want_dead))
    print(f'核对 {checked} 个 (网元, MOC) 组合')
    if has_hist_row:
        if bad_row:
            print(f'\nhist_row 不一致 {len(bad_row)} 个：')
            for ne, moc, a, wa, d, wd in bad_row[:20]:
                print(f'  {ne} {moc:12} 活着 {a:>7,} / 应 {wa:>7,}；死行 {d:>7,} / 应 {wd:>7,}')
        else:
            print('hist_row 一致：活着 == 2026 键数；死行 == |2023 \\ 2026| ✓')
    if not bad and not bad_row:
        print('全部一致：cur 的键集合 == 2026 批的键集合 ✓')
        return 0
    print(f'\n不一致 {len(bad)} 个：')
    for ne, moc, ncur, nwant, nextra, nmiss in bad[:30]:
        print(f'  {ne} {moc:12} cur {ncur:>7,} / 2026 {nwant:>7,}  '
              f'僵尸 {nextra:>7,}  丢行 {nmiss:>7,}')
    return 1


if __name__ == '__main__':
    sys.exit(main())
