# -*- coding: utf-8 -*-
"""提升列缺省值（A4）：从 6 份实测原文算"网元内众数"，冻成 `defaults.tsv`。

移植自 `ims_mem/codegen/gen_defaults.py`（只保留缺省值那部分，去掉 HTML 审核表），差别：
  * 输出 `defaults.tsv`（`MENAME\tMOC\tPARAM\tVALUE`）而不是 JSON —— core 不引 serde；
  * PRA 三对象**不做归一**（本仓 PRATG/PRALNK/PRATKC 是独立表；实测三者都没有提升参数，
    所以归一与否结果相同，但保持"哪张表的参数归哪张表"更安全）；
  * 额外做三份交叉比对，全部打印出来：
      a) `ims_mem/defaults.json`（850 条，若存在）—— 逐条比，证明两边一致；
      b) `rust_libraru.db` 的 `ne_moc_default`（969 行）—— 有多少条能对上；
      c) `table_columns.json`（冻结基线）—— 本脚本给的缺省值是否与基线"默认值"一致。

用法：
    python tools/gen_defaults.py           # 只算 + 比对，不写文件
    python tools/gen_defaults.py --write    # 写 defaults.tsv
退出码 0 = 生成成功（比对有不一致只报告，不失败 —— 那正是审核要看的）。
"""
import collections
import glob
import io
import json
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FILES = sorted(glob.glob(os.path.join(ROOT, '..', 'agcf_data', 'ALLME_*.txt')))
OUT = os.path.join(ROOT, 'defaults.tsv')
IMSMEM_JSON = os.path.join(ROOT, '..', 'ims_mem', 'defaults.json')
BASELINE = os.path.join('C:/dev/ims_ws', 'data', 'ims_asset', 'table_columns.json')
BASE_DB = os.path.join('C:/dev/tryit', 'rust_libraru.db')
DECL_SRC = [os.path.join(ROOT, 'crates', 'core', 'src', 'schema_gen_data.rs')]
DECL_DIR = os.path.join(ROOT, 'crates', 'core', 'src', 'schema')

USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ME_LINE = re.compile(rb'ADD ME:')
MML = re.compile(rb'^(ADD|MOD|SET)\s+([A-Z0-9]+):')
DECL_BLOCK = re.compile(r'mml_table!\s*\{\s*\n\s*([A-Z0-9_]+)\s*,\s*\w+\s*,\s*\n(.*?)\n\}', re.S)
FIELD = re.compile(r'^\s{4}(?:pub )?(?:r#)?([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*([A-Za-z][A-Za-z0-9_]*)\s*(?:\([^)]*\))?\s*(?:\[(\d+)\])?\s*,', re.M)

# 被 shim 直接消费、**不进提升列**的参数（与 D27 / D50 / D54 一致）：
#   ASBR.PRI                            → AsbrShim 即时比对 pui_num（D27：PRI ≡ PUI，不建行/不驻留）
#   PRATKC.{SC,EC,TID,CS,TIDPFX,TGN,MN,TG} → PratkcShim 聚合成 PCM 块（D54）
#   SRTCOST.{CST,PSR,SR,SRN}            → SrtcostShim 聚合成成本方案（D50）
# 这些参数在各表里都"出现过"，但本仓不把它们当未声明参数（有意义的值已经在行/块里）。
SHIM_CONSUMED = {
    'ASBR': {'PRI'},
    'PRATKC': {'SC', 'EC', 'TID', 'CS', 'TIDPFX', 'TGN', 'MN', 'TG'},
    'SRTCOST': {'CST', 'PSR', 'SR', 'SRN'},
}


def split_params(body):
    out = []
    i = 0
    inq = False
    start = 0
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


def vstr(v):
    try:
        return v.decode('gbk')
    except Exception:  # noqa: BLE001
        return v.hex()


def parse_declared():
    """{MOC: set(已声明参数)}；参数组 `U16[28]` 展开成 NAME1..NAME28。"""
    out = {}
    srcs = list(DECL_SRC) + sorted(glob.glob(os.path.join(DECL_DIR, '*.rs')))
    for path in srcs:
        if not os.path.exists(path):
            continue
        text = io.open(path, encoding='utf-8').read()
        for m in DECL_BLOCK.finditer(text):
            moc, body = m.group(1), m.group(2)
            names = out.setdefault(moc, set())
            for fm in FIELD.finditer(body):
                name, _ty, width = fm.group(1), fm.group(2), fm.group(3)
                if width:
                    for i in range(1, int(width) + 1):
                        names.add(f'{name}{i}'.upper())
                else:
                    names.add(name.upper())
    return out


def main(argv):
    write = '--write' in argv
    check = '--check' in argv
    if not FILES:
        print('找不到 ../agcf_data/ALLME_*.txt', file=sys.stderr)
        return 2
    declared = parse_declared()
    print(f'本仓声明 {len(declared)} 张表（用于判定"未声明参数"）')

    # stats[ne][moc][param] = Counter(value)
    stats = collections.defaultdict(lambda: collections.defaultdict(lambda: collections.defaultdict(collections.Counter)))
    rows_by = collections.Counter()
    for path in FILES:
        data = open(path, 'rb').read()
        meid2name = {}
        cur = None
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r; \t')
            if not line:
                continue
            m = USE_ME.search(line)
            if m:
                cur = meid2name.get(int(m.group(1)))
                continue
            if ME_LINE.search(line):
                body = line[line.find(b':') + 1:]
                meid = name = None
                for (k, v) in split_params(body):
                    if k == 'MEID':
                        try:
                            meid = int(unq(v))
                        except ValueError:
                            pass
                    elif k == 'MENAME':
                        name = vstr(unq(v))
                if meid is not None and name is not None:
                    meid2name[meid] = name
                continue
            m = MML.match(line)
            if not m or cur is None:
                continue
            obj = m.group(2).decode()
            if obj == 'ME':
                continue
            rows_by[(cur, obj)] += 1
            for (p, v) in split_params(line[m.end():]):
                stats[cur][obj][p][unq(v)] += 1

    # 只留未声明参数
    lines = []
    per_moc = collections.Counter()
    for ne in sorted(stats):
        for obj in sorted(stats[ne]):
            decl = declared.get(obj)
            if decl is None:
                continue  # 未建模对象
            for param, vals in sorted(stats[ne][obj].items()):
                if param in decl or param in SHIM_CONSUMED.get(obj, ()):
                    continue
                mode, _cnt = vals.most_common(1)[0]
                lines.append((ne, obj, param, vstr(mode)))
                per_moc[obj] += 1

    print(f'扫出提升参数 {len(lines)} 条（{len(per_moc)} 张表 / {len(stats)} 个网元）')
    print('  按表 top10：', per_moc.most_common(10))

    # ---- a) 与 ims_mem/defaults.json 逐条比 ----
    if os.path.exists(IMSMEM_JSON):
        j = json.load(io.open(IMSMEM_JSON, encoding='utf-8'))
        theirs = {(ne, moc, p): v for ne, mocs in j.items() for moc, ps in mocs.items() for p, v in ps.items()}
        ours = {(ne, moc, p): v for (ne, moc, p, v) in lines}
        same = sum(1 for k in ours if theirs.get(k) == ours[k])
        only_ours = [k for k in ours if k not in theirs]
        only_theirs = [k for k in theirs if k not in ours]
        diff = [k for k in ours if k in theirs and theirs[k] != ours[k]]
        print(f'\n与 ims_mem/defaults.json 比：共 {len(theirs)} 条 / 我们 {len(ours)} 条；'
              f'一致 {same} / 值不同 {len(diff)} / 只在ours {len(only_ours)} / 只在theirs {len(only_theirs)}')
        for k in diff[:5]:
            print(f'   值不同 {k}: 我们「{ours[k]}」 vs ims_mem「{theirs[k]}」')
        for k in only_ours[:5]:
            print(f'   只在ours {k} = {ours[k]}')
        for k in only_theirs[:5]:
            print(f'   只在theirs {k} = {theirs[k]}')
    else:
        print(f'\n⚠ 没有 {IMSMEM_JSON}，跳过与 ims_mem 的比对')

    # ---- b) 与 ne_moc_default 交叉 ----
    if os.path.exists(BASE_DB):
        db = sqlite3.connect(BASE_DB)
        name2neid = {r[0]: r[1] for r in db.execute('SELECT mename, ne_id FROM ne')}
        nmd = {}
        for neid, moc, _g, param, value in db.execute(
                'SELECT ne_id, moc, ifnull(grp,""), param, value FROM ne_moc_default'):
            nmd.setdefault((neid, moc.upper(), param.upper()), set()).add(value)
        hit = miss = bad = 0
        for (ne, moc, param, value) in lines:
            neid = name2neid.get(ne)
            vals = nmd.get((neid, moc, param)) if neid is not None else None
            if vals is None:
                miss += 1
            elif any(str(v) == value for v in vals):
                hit += 1
            else:
                bad += 1
        print(f'与 ne_moc_default（{len(nmd)} 个键）交叉：命中 {hit} / 与库值不同 {bad} / 库里没有 {miss}')
    else:
        print(f'⚠ 没有 {BASE_DB}，跳过 ne_moc_default 交叉')

    # ---- c) 与 table_columns.json（冻结基线）交叉 ----
    # 基线每条参数带 `verdict`（默认表 / 列 / 稀疏 diff / 默认+diff）与 `key` 标记。
    # 健康的形状：提升清单里**不该**出现 verdict='列'（那是实体列），也不该出现 key=True。
    if os.path.exists(BASELINE):
        rows = json.load(io.open(BASELINE, encoding='utf-8'))
        base = {(r['moc'], r['param']): r for r in rows}
        verdicts = collections.Counter()
        as_col = as_key = absent = 0
        for (_ne, moc, param, _v) in lines:
            r = base.get((moc, param))
            if r is None:
                absent += 1
                continue
            verdicts[r['verdict']] += 1
            if r['verdict'] == '列':
                as_col += 1
            if r.get('key'):
                as_key += 1
        print(f'与 table_columns.json 交叉：verdict {dict(verdicts)} / 基线里没有 {absent}')
        print(f'   其中被判「列」{as_col} 条、「键」{as_key} 条（都应为 0 —— 实体列/键不该进提升清单）')
    else:
        print(f'⚠ 没有 {BASELINE}，跳过 table_columns 交叉')

    if write:
        with io.open(OUT, 'w', encoding='utf-8', newline='\n') as f:
            f.write('# @generated by tools/gen_defaults.py —— MENAME\tMOC\tPARAM\tVALUE\n')
            for (ne, moc, param, value) in lines:
                v = value.replace('\\', '\\\\').replace('\t', '\\t').replace('\n', '\\n')
                f.write(f'{ne}\t{moc}\t{param}\t{v}\n')
        print(f'\n写出 {OUT}：{len(lines)} 条')

    # ---- 可复现性：重算结果 == 磁盘上的 defaults.tsv ----
    if check:
        if not os.path.exists(OUT):
            print(f'\n✗ 没有 {OUT}（先跑 --write）')
            return 1
        disk = set()
        for line in io.open(OUT, encoding='utf-8'):
            if line.startswith('#') or not line.strip():
                continue
            parts = line.rstrip('\n').split('\t')
            if len(parts) == 4:
                disk.add(tuple(parts))
        now = {(ne, moc, param, value.replace('\\', '\\\\').replace('\t', '\\t').replace('\n', '\\n'))
               for (ne, moc, param, value) in lines}
        same = len(now & disk)
        print(f'\n复现校验：重算 {len(now)} 条 / 磁盘 {len(disk)} 条 / 一致 {same} / '
              f'只在重算 {len(now - disk)} / 只在磁盘 {len(disk - now)}')
        for k in list(now - disk)[:5]:
            print(f'   只在重算 {k}')
        for k in list(disk - now)[:5]:
            print(f'   只在磁盘 {k}')
        return 0 if now == disk else 1
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
