# -*- coding: utf-8 -*-
"""把 ims_mem 的 `schema_gen_data.rs`（143 条已验证声明）转成 ims_mem2 的 `mml_table!` 声明。

为什么用转换而不是重写生成器：那 143 条声明是 ims_mem 从 `agcf_mml.db` + 6 份实测推导出来、
并用 33 表对账验过的资产；重写推导逻辑等于重赌一遍键推断。转换只做**类型映射**，
风险面从"推断算法"缩到"类型表"。

类型映射（P0.4b 阶段）：
    u8/u16/u32/Bool/Text/Enum/Pfx   → 同名（1:1）
    Eid / Ip / Spc / Pair / Bits    → Text（原文驻留；数值/位图语义待 P0.5）
    PfxText / Uri                   → Text（本条声明里未出现，留作兜底）

用法：
    python tools/port_schema.py --in ..\\ims_mem\\src\\schema_gen_data.rs --out crates\\core\\src\\schema_gen_data.rs
"""
import argparse
import os
import re
import sqlite3
import sys

TYPE_MAP = {
    'u8': 'U8',
    'u16': 'U16',
    'u32': 'U32',
    'u64': 'U64',          # 未实现，遇到就报错
    'Bool': 'Bool',
    'Text': 'Text',
    'Enum': 'Enum',
    'Pfx': 'Pfx',
    'PfxText': 'Text',
    # A2（撤销 D24 的降级）：这四类升级成真类型（行内仍是 4 B，宽值池里放 u64）
    'Eid': 'Eid',
    'Ip': 'Ip',
    'Spc': 'Spc',
    'Pair': 'Pair',
    'Bits': 'Bits',
    'Uri': 'Text',
}

# ---- A2：位名表（`BTn` 常量 → `BITS_TABLES`）----
# ims_mem 的生成物里每个位串列长得像 `eva: Bits(&BT0)`，而 `BT0` 的定义就在同一个文件里：
#   `pub static BT0: BitNameTable = BitNameTable { param: "APRATGDEF.EVA", names: &["AEC", ...] };`
# 所以直接解析 BTn（**已经是按 (MOC,PARAM) 解析好的**），不需要再查 `agcf_mml.db`。
# 注意：同名参数在不同 MOC 下位名顺序确实不同（实测 APRATGDEF.SVRCTRLS 3 位、SVRCTRLS 31 位），
# 因此必须按 (MOC,PARAM) 分表，不能只按参数名。
BT_ORDER = []     # ["BT0", "BT1", ...]
BT_IDX = {}       # "BT0" -> 0
BT_NAMES = {}     # "BT0" -> ["AEC", ...]
BT_PARAM = {}     # "BT0" -> "APRATGDEF.EVA"
BT_DEF = re.compile(
    r'pub static (BT\d+):\s*BitNameTable\s*=\s*BitNameTable\s*\{\s*param:\s*"([^"]*)"\s*,\s*names:\s*&\[([^\]]*)\]\s*\};')


def load_bits_tables(src_text):
    for m in BT_DEF.finditer(src_text):
        name, param, names = m.group(1), m.group(2), m.group(3)
        lst = [x.strip().strip('"') for x in names.split(',') if x.strip()]
        if not lst:
            continue
        BT_ORDER.append(name)
        BT_IDX[name] = len(BT_ORDER) - 1
        BT_NAMES[name] = lst
        BT_PARAM[name] = param
    print(f'位名表 {len(BT_ORDER)} 张（从 ims_mem 的 BTn 常量解析）')


def bits_tables_rs():
    out = ['/// A2：位名表（下标 = `Bits(n)` 里的 n；位序即声明序，按 `(MOC, PARAM)` 分表）',
           'pub static BITS_TABLES: &[&[&str]] = &[']
    for name in BT_ORDER:
        names = ', '.join('"%s"' % n for n in BT_NAMES[name])
        out.append(f'    // {BT_PARAM[name]}')
        out.append(f'    &[{names}],')
    out.append('];')
    return '\n'.join(out)

FIELD = re.compile(
    r'^\s*(tmp |agg )?((?:r#)?[a-z0-9_]+)\s*:\s*([A-Za-z][A-Za-z0-9_]*)'
    r'\s*(\([^)]*\))?\s*(\[[^\]]*\])?\s*(?:as\s+([A-Z0-9_]+))?\s*,\s*$'
)
KEY = re.compile(r'^\s*key\s*\[(.*)\]\s*,\s*$')
HEAD = re.compile(r'^\s*([A-Z][A-Z0-9_]*)\s*,\s*([A-Z][A-Za-z0-9_]*)\s*,\s*$')

# ---- A2 的**保留 Text 例外**（升级成 Eid 会让两条机件失效，属 A2.5 的活）----
#   * `MGW.EID` 是 MGW 的**键列**，而且行里可能有 raw 兜底（实测 6 个坏形状里有 `cdma_ss6.com`）；
#     键列一旦允许两种形态（typed | 原文），`key_of` 与键索引都要改。
#   * `N7TG.EID` / `SG.EID` / `V5TG.MGEID` / `V5TKC.MGEID` 是 Ref 边的**源列**：
#     typed 值只能从宽值池取（`col_sym` 拿不到），refs 要改成 wide-aware。
#   * `ASBR.EID` / `PRATG.MGW` 是手写表里的 Text，本来就是另一端，A2.5 一起升。
# 其余 86 列（Ip 41 + Bits 44 + MGWID.EID）本轮升级。
# A2.5 起这 5 列也升级了（键支持 typed|raw 双形态、refs 支持宽值比较）。
# 这里保留空集合作为机制：将来再遇到装不下的类型，往这里加即可。
KEEP_TEXT = set()

# D53 步骤 2：`N7TKC` 改成稀疏 `Circuit`（手写在 `src/n7.rs`），不再作为长尾表生成。
# 它的 9 列契约（列名/列序/键）在 n7.rs 里用同样的 `mml_table!` 声明，所以
# `tools/gen_refs.py` 与投影看到的列号完全不变。
SKIP_MOCS = {'N7TKC'}

# D53 步骤 2：`N7TG` 末尾追加两个句柄列（**追加在末尾**，既有 83 列的列号一个都不动，
# 否则 refs_gen 里 N7TG 的 6 条边（CSC/EID/LMTGRP/RCHS/SOPC/SRC）会集体错位）。
EXTRA_COLS = {'N7TG': [('circ_start', 'U32'), ('circ_len', 'U16')]}


def parse(text):
    """→ [(moc, ty, keys, [(name, ims_type)])]"""
    out = []
    i = 0
    lines = text.splitlines()
    while i < len(lines):
        if lines[i].strip() != 'mml_table! {':
            i += 1
            continue
        i += 1
        moc = ty = None
        keys = []
        fields = []
        depth = 1
        while i < len(lines) and depth > 0:
            ln = lines[i]
            st = ln.strip()
            if st.startswith('}'):
                depth -= 1
                i += 1
                break
            i += 1
            if not st or st.startswith('//'):
                continue
            m = HEAD.match(ln)
            if m and moc is None:
                moc, ty = m.group(1), m.group(2)
                continue
            m = KEY.match(ln)
            if m:
                keys = [k.strip() for k in m.group(1).split(',') if k.strip()]
                continue
            m = FIELD.match(ln)
            if m:
                tmp_kind, name, t, _args, _arr, alias = m.groups()
                if tmp_kind:
                    # tmp/agg 在 ims_mem 里"不进行/不解析"，这里直接跳过（P0.5 再补）
                    continue
                args = (_args or "()")[1:-1].strip()
                fields.append((alias.lower() if alias else name, t, args))
                continue
            if st.startswith('branch '):
                raise SystemExit(f'{moc}: branch 语法暂不支持（{st}）')
            if st.startswith('_ =>'):
                continue
            raise SystemExit(f'{moc}: 无法解析的行: {st}')
        if moc:
            out.append((moc, ty, keys, fields))
    return out


WIDE_TOO_WIDE = []   # 位名 > 64，装不进 u64，保留 Text


def convert(decls):
    unmapped = {}
    body = []
    for moc, ty, keys, fields in decls:
        # D53 步骤 2：句柄列追加在末尾（列号不漂）；名/类型直接用 mml_table! 的口径
        extra = EXTRA_COLS.get(moc, [])
        lines = [f'// {moc} —— {len(fields) + len(extra)} 列' + (f'，键 {keys}' if keys else '，无键')
                 + ('（末 2 列是 D53 的电路句柄）' if extra else '')]
        lines.append('mml_table! {')
        lines.append(f'    {moc}, {ty},')
        if keys:
            lines.append(f'    key [{", ".join(keys)}],')
        for name, t, args in fields:
            mt = TYPE_MAP.get(t)
            if (moc, name) in KEEP_TEXT:
                mt = 'Text'
            if mt is None:
                raise SystemExit(f'{moc}.{name}: 未知类型 {t}')
            if mt != 'Text' and t in ('Eid', 'Ip', 'Spc', 'Pair', 'Bits', 'PfxText', 'Uri'):
                pass
            if t != mt:
                unmapped[t] = unmapped.get(t, 0) + 1
            if mt == 'U64':
                raise SystemExit(f'{moc}.{name}: U64 尚未在宏里实现')
            if t == 'Bits':
                bt = args.strip().lstrip('&')
                idx = BT_IDX.get(bt)
                if idx is None:
                    raise SystemExit(f'{moc}.{name}: 位名表 {args} 找不到（BTn 常量缺失）')
                if len(BT_NAMES[bt]) > 128:
                    # 位名超过 64 个装不进 u64 → 保留 Text（CVSTATYPE.INTRA 有 120 位）
                    WIDE_TOO_WIDE.append(f'{moc}.{name}({len(BT_NAMES[bt])}位)')
                    mt = 'Text'
                else:
                    mt = f'Bits({idx})'
            lines.append(f'    {name}: {mt},')
        for name, mt in extra:
            lines.append(f'    {name}: {mt},')
        lines.append('}')
        lines.append('')
        body.append('\n'.join(lines))
    return '\n'.join(body), unmapped


HEADER = '''// @generated by tools/port_schema.py —— 请勿手改
// 长尾 MOC 的声明（143 张），由 ims_mem 的 `schema_gen_data.rs` 转换而来。
//
// 类型映射：u8/u16/u32/Bool/Text/Enum/Pfx 1:1；
// Eid/Ip/Spc/Pair/Bits 暂按 Text 原文驻留（数值/位图语义待 P0.5，见 README 已知缺口）。
//
// 每张表都实现了 TableApi（Table<H> 的 blanket impl），所以运行期用
// gen_moc_index() + new_gen_table() 惰性建表。
//
// ⚠ D53 步骤 2：`N7TKC` 不在这里 —— 它改成稀疏 `Circuit` 存储（手写在 `src/n7.rs`，
// 那里的声明与这里的列名/列序一致，所以列号不变）；`N7TG` 末尾追加了
// `circ_start/circ_len` 两个句柄列（`EXTRA_COLS`）。

use crate::mml_table;
use crate::table::TableApi;
'''


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--in', dest='src', required=True)
    ap.add_argument('--out', dest='dst', required=True)
    args = ap.parse_args()

    text = open(args.src, encoding='utf-8').read()
    load_bits_tables(text)
    decls = parse(text)
    skipped = [d[0] for d in decls if d[0] in SKIP_MOCS]
    decls = [d for d in decls if d[0] not in SKIP_MOCS]
    if len(decls) + len(skipped) != 143:
        print(f'!! 解析到 {len(decls) + len(skipped)} 条声明（期望 143）', file=sys.stderr)
    if skipped:
        print(f'按 D53 步骤 2 跳过（改走稀疏 Circuit）：{skipped}')

    decls.sort(key=lambda d: d[0])
    body, unmapped = convert(decls)

    mocs = [d[0] for d in decls]
    tys = [d[1] for d in decls]

    lines = [HEADER, '', body, '']
    lines.append('/// 全部长尾 MOC（升序；`gen_moc_index` 二分查找）')
    lines.append('pub static MOCS: &[&str] = &[')
    for m in mocs:
        lines.append(f'    "{m}",')
    lines.append('];')
    lines.append('')
    lines.append('/// MOC 名（大小写不敏感）→ 表 id')
    lines.append('pub fn gen_moc_index(name: &[u8]) -> Option<usize> {')
    lines.append('    if name.is_empty() || name.len() > 16 {')
    lines.append('        return None;')
    lines.append('    }')
    lines.append('    let mut buf = [0u8; 16];')
    lines.append('    for (i, &c) in name.iter().enumerate() {')
    lines.append('        buf[i] = c.to_ascii_uppercase();')
    lines.append('    }')
    lines.append('    MOCS.binary_search_by(|m| m.as_bytes().cmp(&buf[..name.len()]))')
    lines.append('        .ok()')
    lines.append('}')
    lines.append('')
    lines.append('/// 按 id 建一张空表')
    lines.append(bits_tables_rs())
    lines.append('')
    lines.append('pub fn new_gen_table(id: usize) -> Option<Box<dyn TableApi>> {')
    lines.append('    Some(match id {')
    for i, ty in enumerate(tys):
        lines.append(f'        {i} => Box::new({ty}Table::new()),')
    lines.append('        _ => return None,')
    lines.append('    })')
    lines.append('}')
    lines.append('')

    open(args.dst, 'w', encoding='utf-8', newline='\n').write('\n'.join(lines))
    print(f'写出 {args.dst}：{len(decls)} 张表')
    if unmapped:
        print('类型降级统计：', unmapped)


if __name__ == '__main__':
    main()
