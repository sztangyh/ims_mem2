# -*- coding: utf-8 -*-
"""A3 独立校验：投影里的 `*_NUM` 伴生列是否等于**原文列的字冠数字**？

独立实现（不调用 Rust 的任何规范形代码）：从 `cur_<MOC>` 读出原文列与伴生列，
用本脚本自己的规则把原文折算成规范文本，逐行比较。

规则（与 `frombytes::render_pfx` 一致）：`K'` + 数字的大写十六进制写法
（`0-9` 原样、`A-E` 大写、`*`→`E`、`#`→`F`）。
"""
import collections
import io
import os
import re
import sqlite3
import sys

ROOT = r'C:\dev\tryit\ims_mem2'
def _cur_db():
    """export 实际写出的库（被占用时会换名，路径记在 temp/cur_db.txt）。"""
    p = os.path.join(ROOT, 'temp', 'cur_db.txt')
    try:
        with open(p, encoding='utf-8') as f:
            v = f.read().strip()
        if v:
            return v if os.path.isabs(v) else os.path.join(ROOT, v)
    except OSError:
        pass
    return os.path.join(ROOT, 'temp', 'proj.db')


DB = _cur_db()
DECL = re.compile(r'mml_table!\s*\{\s*\n\s*([A-Z0-9_]+)\s*,\s*\w+\s*,\s*\n(.*?)\n\}', re.S)
FIELD = re.compile(r'^\s{4}([a-z_0-9]+):\s*(Pfx|PfxText),', re.M)


def decls():
    srcs = [os.path.join(ROOT, 'crates', 'core', 'src', 'schema_gen_data.rs')]
    d = os.path.join(ROOT, 'crates', 'core', 'src', 'schema')
    srcs += [os.path.join(d, f) for f in sorted(os.listdir(d)) if f.endswith('.rs')]
    out = collections.defaultdict(list)
    for path in srcs:
        if not os.path.exists(path):
            continue
        text = io.open(path, encoding='utf-8').read()
        for m in DECL.finditer(text):
            for fm in FIELD.finditer(m.group(2)):
                out[m.group(1)].append(fm.group(1).upper())
    return out


def canon(raw):
    if raw is None:
        return None
    s = raw
    if len(s) >= 2 and s[0] in 'Kk' and s[1] == "'":
        s = s[2:]
    out = []
    for ch in s:
        if ch.isdigit():
            out.append(ch)
        elif ch in 'Aa':
            out.append('A')
        elif ch in 'Bb':
            out.append('B')
        elif ch in 'Cc':
            out.append('C')
        elif ch in 'Dd':
            out.append('D')
        elif ch in 'Ee' or ch == '*':
            out.append('E')
        elif ch == '#':
            out.append('F')
        else:
            return None  # 非法字符（按约定不该出现）
    return "K'" + ''.join(out)


def main():
    if not os.path.exists(DB):
        print(f'没有 {DB}（先跑 export）', file=sys.stderr)
        return 2
    db = sqlite3.connect(DB)
    have = {r[0] for r in db.execute("select name from sqlite_master where type='table'")}
    tot = bad = empty = 0
    detail = []
    checked = 0
    for moc, cols in sorted(decls().items()):
        tbl = f'cur_{moc}'
        if tbl not in have:
            continue
        for c in cols:
            num = c + '_NUM'
            names = {r[1] for r in db.execute(f'PRAGMA table_info("{tbl}")')}
            if c not in names or num not in names:
                continue
            checked += 1
            n_bad = 0
            n_empty = 0
            for raw, numv in db.execute(f'SELECT "{c}", "{num}" FROM "{tbl}"'):
                tot += 1
                if raw is None:
                    if numv is not None:
                        n_bad += 1
                    continue
                want = canon(raw)
                if want is None:
                    # 非法字冠：伴生列应为空（不算错）
                    if numv is not None:
                        n_empty += 1
                    continue
                if numv != want:
                    n_bad += 1
                    if len(detail) < 6:
                        detail.append((moc, c, raw, numv, want))
            bad += n_bad
            empty += n_empty
            if n_bad:
                print(f'  ✗ {moc}.{c}: {n_bad} 行不符')
    print(f'检查 {checked} 对列 / 比较 {tot} 个单元格：不符 {bad} / 非法字冠 {empty}')
    for d in detail:
        print('   样例', d[0], d[1], '原文', d[2], '伴生列', d[3], '应为', d[4])
    return 0 if bad == 0 else 1


if __name__ == '__main__':
    sys.exit(main())
