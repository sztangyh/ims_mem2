# -*- coding: utf-8 -*-
"""键推导复算（A5）：从 6 份实测原文**重新推导**每张长尾表的键，与本仓声明逐张比对。

移植自 `ims_mem/codegen/gen_desc.py` 的 `scan_files` / `choose_key`（2026-10-20 那套：
"文档验证 > RMV 实证 > 单文件唯一 > 双参组合 > 单例 > 无键"），只有两处差别：
  * 输入固定为本仓的 `../agcf_data/ALLME_*.txt`（不依赖 ims_mem 的绝对路径）；
  * 产出不是生成代码，而是"推导键 vs 声明键"的对照报告（退出码 0 = 全一致）。

为什么需要它（A5）：本仓 142 张长尾表的键是 `port_schema.py` 从 ims_mem 的生成物**抄**来的
（D23 的取舍），一旦要接新 MOC，就没有"键从哪来"的可复算依据。这个脚本补上那一环，
并顺带**证明**当年抄过来的键与重新推导的结果一致。

用法：
    python tools/infer_keys.py              # 扫 ../agcf_data/ALLME_*.txt
    python tools/infer_keys.py --verbose    # 打印每张表的推导来源
退出码：0 = 全部一致（推导 == 声明，允许顺序不同）；1 = 有差异；2 = 输入缺失。
"""
import collections
import glob
import itertools
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FILES = sorted(glob.glob(os.path.join(ROOT, '..', 'agcf_data', 'ALLME_*.txt')))
DOC_DB = os.path.join(ROOT, '..', 'mml_doc', 'agcf_doc', 'agcf_mml.db')
GEN = os.path.join(ROOT, 'crates', 'core', 'src', 'schema_gen_data.rs')
EXTRA_DECL = [
    os.path.join(ROOT, 'crates', 'core', 'src', 'schema', 'n7.rs'),
]

# 键剔除白名单（与 gen_desc.py 一致）：引用名称型冗余键不进合并键。
# 依据：`tools/check_refs.py` 实测"名称列 ≡ f(数值键)"全网 0 不一致（2026-11-12 复验 475,569 条）。
KEY_DROP = {'SRTATTR': {'RN', 'SRN'}}

# 领域偏好键序（编号类键先于名称类冗余键）——与 gen_desc.py 逐字一致
PREF = ['TG', 'MN', 'PLN', 'LNKN', 'DSP', 'DCX', 'SC', 'SEN', 'EID', 'TGID', 'SRC', 'R', 'RAN',
        'DID', 'LP', 'PFX', 'CID', 'IMPU', 'PUI', 'SN', 'SR', 'CSC', 'TID', 'RN', 'ID', 'O',
        'I', 'EI', 'SVRNAME']

moc_re = re.compile(rb'/\* MOC = ([A-Z0-9]+) \*/')
opcmd_re = re.compile(rb'^([A-Z]+) ([A-Z0-9]+):')
DECL_BLOCK = re.compile(r'mml_table!\s*\{\s*\n\s*([A-Z0-9_]+)\s*,\s*\w+\s*,\s*\n(.*?)\n\}', re.S)
KEY_LINE = re.compile(r'^\s*key\s*\[([^\]]*)\]', re.M)


# ---------------- 一、扫原文，累计统计（与 gen_desc.py 同构） ----------------
class ParamStat:
    __slots__ = ('n', 'distinct', 'per_file')

    def __init__(self):
        self.n = 0
        self.distinct = set()
        self.per_file = collections.defaultdict(set)


class MocStat:
    def __init__(self):
        self.params = {}
        self.rmvp = collections.Counter()
        self.addrows = self.setrows = self.modrows = self.rmvrows = 0
        self.rows_by_file = collections.defaultdict(lambda: collections.Counter())


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
                out.append((seg[:eq].strip(), seg[eq + 1:].strip()))
            start = i + 1
        i += 1
    seg = body[start:]
    eq = seg.find(b'=')
    if eq > 0:
        out.append((seg[:eq].strip(), seg[eq + 1:].strip()))
    return out


def scan_files(paths):
    stats = {}
    for fid, path in enumerate(paths):
        data = open(path, 'rb').read()
        moc = None
        for line in data.split(b'\n'):
            line = line.strip()
            m = moc_re.match(line)
            if m:
                moc = m.group(1).decode()
                continue
            m = opcmd_re.match(line)
            if not m or moc is None:
                continue
            op, obj = m.group(1).decode(), m.group(2).decode()
            st = stats.setdefault(obj, MocStat())
            if op == 'ADD':
                st.addrows += 1
            elif op == 'MOD':
                st.modrows += 1
            elif op == 'SET':
                st.setrows += 1
            elif op == 'RMV':
                st.rmvrows += 1
            st.rows_by_file[fid][op] += 1
            ps = split_params(line[m.end():].rstrip(b';'))
            seen = set()
            for k, v in ps:
                k = k.decode('ascii', 'replace').upper()
                v = v.rstrip(b';')
                if v[:1] == b'"' and v[-1:] == b'"':
                    v = v[1:-1]
                p = st.params.get(k)
                if p is None:
                    p = st.params[k] = ParamStat()
                p.n += 1
                p.distinct.add(v)
                p.per_file[fid].add(v)
                seen.add(k)
                if op == 'RMV':
                    st.rmvp[k] += 1
            _ = seen
    return stats


# ---------------- 二、文档验证键（composite_keys） ----------------
def load_doc_keys():
    """返回 {moc: set(key params)}；db 不在就返回 None（脚本会标注"跳过文档验证"）。"""
    if not os.path.exists(DOC_DB):
        return None
    con = sqlite3.connect(DOC_DB)
    ck = {}
    for cid, kp in con.execute('SELECT command_id, key_params FROM composite_keys'):
        row = con.execute('SELECT cmd_name FROM commands WHERE id=?', (cid,)).fetchone()
        if not row or not row[0]:
            continue
        op, _, moc = row[0].partition(' ')
        if op != 'ADD':
            continue
        keys = [g.split('|')[0].strip() for g in (kp or '').split(',')]
        keys = [k for k in keys if k]
        if keys:
            ck[moc] = set(k.upper() for k in keys)
    return ck


# ---------------- 三、键推导（与 gen_desc.py::choose_key 同逻辑） ----------------
def perfile_unique(st, op, k):
    p = st.params.get(k)
    if p is None:
        return False
    for fid, opcnt in st.rows_by_file.items():
        rows = opcnt.get(op, 0)
        if rows == 0:
            continue
        if len(p.per_file.get(fid, ())) != rows:
            return False
    return True


def choose_key(moc, st, ck):
    if ck and moc in ck:
        return sorted(ck[moc]), 'doc_verified'
    if st.rmvrows > 0 and st.rmvp:
        keys = sorted([k for k, c in st.rmvp.items() if c >= st.rmvrows * 0.9])
        if keys:
            return keys, 'rmv_empirical'
    main_op = 'ADD' if st.addrows else ('SET' if st.setrows else ('MOD' if st.modrows else None))
    if not main_op:
        return [], 'none'
    # 单例判据：SET/MOD 主操作每文件至多 1 行 → 任何参数"文件内唯一"都是空真
    if main_op in ('SET', 'MOD') and not any(
            cnt.get(main_op, 0) > 1 for cnt in st.rows_by_file.values()):
        return [], 'singleton'
    total = getattr(st, f'{main_op.lower()}rows')
    present100 = [k for k, p in st.params.items() if p.n >= total * 0.999]
    for k in [p for p in PREF if p in present100] + sorted(set(present100) - set(PREF)):
        if perfile_unique(st, main_op, k):
            return [k], f'unique_{main_op.lower()}_single'
    if main_op == 'ADD' and len(present100) <= 40:
        for a, b in itertools.combinations(sorted(present100), 2):
            pa, pb = st.params[a], st.params[b]
            ok_all = True
            for fid, opcnt in st.rows_by_file.items():
                rows = opcnt.get('ADD', 0)
                if rows == 0:
                    continue
                sa, sb = pa.per_file.get(fid, ()), pb.per_file.get(fid, ())
                if len(sa) * len(sb) < rows or len(sa) * len(sb) > rows * 30:
                    ok_all = False
                    break
                if len(sa) != rows and len(sb) != rows and len(sa) * len(sb) != rows:
                    ok_all = False
                    break
            if ok_all and st.addrows > 0:
                return [a, b], 'empirical_pair'
    if main_op != 'ADD':
        return sorted(st.params.keys()), 'mod_set_all'
    return [], 'no_unique_key'


# ---------------- 四、本仓声明里的键 ----------------
def parse_decls():
    out = {}
    srcs = [GEN] + [p for p in EXTRA_DECL if os.path.exists(p)]
    for path in srcs:
        text = open(path, encoding='utf-8').read()
        for m in DECL_BLOCK.finditer(text):
            moc, body = m.group(1), m.group(2)
            km = KEY_LINE.search(body)
            keys = []
            if km:
                # 字段名是 Rust 关键字时写成 #type，比较前去掉前缀（SYSCONF 的 TYPE）
                keys = [k.strip().upper().removeprefix('R#') for k in km.group(1).split(',') if k.strip()]
            out[moc] = keys
    return out


def main(argv):
    verbose = '--verbose' in argv
    if not FILES:
        print('找不到 ../agcf_data/ALLME_*.txt', file=sys.stderr)
        return 2
    stats = scan_files(FILES)
    ck = load_doc_keys()
    if ck is None:
        print(f'⚠ 没有 {DOC_DB} → 跳过"文档验证"这一级（其余级照常推导）')
        ck = {}
    decls = parse_decls()
    print(f'原文 {len(FILES)} 个文件 / MOC {len(stats)} 个；本仓声明 {len(decls)} 张表')

    dist = collections.Counter()
    same, diff, only_decl, only_data = [], [], [], []
    for moc in sorted(set(stats) | set(decls)):
        if moc not in decls:
            only_data.append(moc)
            continue
        if moc not in stats:
            only_decl.append(moc)
            continue
        keys, src = choose_key(moc, stats[moc], ck)
        dist[src] += 1
        drop = KEY_DROP.get(moc)
        derived = [k for k in keys if not (drop and k in drop)] if drop else keys
        declared = decls[moc]
        if set(derived) == set(declared):
            same.append((moc, src, declared, sorted(set(derived) - set(declared))))
            if verbose:
                print(f'  ok  {moc:14} {src:20} 键 {declared}')
        else:
            diff.append((moc, src, sorted(derived), sorted(declared)))

    print('\n推导来源分布：')
    for k, v in dist.most_common():
        print(f'  {k:22} {v}')

    print(f'\n一致 {len(same)} / 不一致 {len(diff)} / 只在声明里 {len(only_decl)} / 只在原文里 {len(only_data)}')
    for moc, src, d, s in diff:
        print(f'  ✗ {moc:14} 推导({src}) {d}  ←→  声明 {s}')
    if only_decl:
        print('  只在声明里（手工表 / 原文没出现）：', ' '.join(only_decl))
    if only_data:
        print('  只在原文里（1 张手写定表 + 8 个 PRA/ASBR 对象：键人工定，不参与推导）：', ' '.join(only_data))
    return 0 if not diff else 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
