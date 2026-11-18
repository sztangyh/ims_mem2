# -*- coding: utf-8 -*-
"""MGW 表声明生成器：`ims_ws/data/mgw/{moc_schema,moc_key}.json` → `crates/core/src/mgw/schema_gen_data.rs`。

为什么是"生成"而不是手写：MGW 侧 59 个 MOC 的列/键/位图/defaults 已经在 ims_core 那边
验证过（`mgw_library.db` 84 表 13,608 行是它的产物，可当金标准），重新推导只会引入分歧。
本脚本只做**形状转换**，不重新判断语义 —— 与 `port_schema.py` 对 AGCF 的 143 张表同一思路。

产物（一个文件，与 AGCF 的 `schema_gen_data.rs` 同构）：

    mml_table! { ATONE, Atone, key [bt, bn, toneid], bt: Text, bn: Text, toneid: Text, }
    mml_table! { SYSINFO, Sysinfo, sysname: Text, ... }        ← single 层（无 key → KEYED=false）

    pub static MGW_MOCS: &[&str]                     ← 排序后的 MOC 名（二分查找）
    pub fn gen_moc_index(name: &[u8]) -> Option<usize>
    pub fn new_gen_table(id: usize) -> Option<Box<dyn TableApi>>
    pub static MGW_LAYER: &[u8]                      ← 每个 MOC 的层（0=dedicated, 1=single）
    pub static MGW_BITMAP: &[&[&str]]                ← 每个 MOC 里需要转"整数位掩码"的参数名
    pub static MGW_DEFAULTS: &[(&str, &str, &str)]   ← (moc, param, value) 全局常量参数

用法：
    python tools/port_mgw_schema.py             # 只算 + 报告
    python tools/port_mgw_schema.py --write     # 写 schema_gen_data.rs
    python tools/port_mgw_schema.py --check     # 重算 == 磁盘（不一致退出码 3）
"""
import argparse
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MGW_DATA = os.path.join('C:/dev/ims_ws', 'data', 'mgw')
OUT = os.path.join(ROOT, 'crates', 'core', 'src', 'mgw', 'schema_gen_data.rs')
NE_OUT = os.path.join(ROOT, 'mgw_ne_map.tsv')

# Rust 关键字：列名撞上就用 r# 转义（宏会剥掉 `r#` 再当参数名）
KEYWORDS = {
    'as', 'break', 'const', 'continue', 'crate', 'dyn', 'else', 'enum', 'extern', 'false',
    'fn', 'for', 'if', 'impl', 'in', 'let', 'loop', 'match', 'mod', 'move', 'mut', 'pub',
    'ref', 'return', 'self', 'static', 'struct', 'super', 'trait', 'true', 'type',
    'unsafe', 'use', 'where', 'while', 'async', 'await', 'box', 'final', 'macro',
    'override', 'priv', 'try', 'typeof', 'unsized', 'virtual', 'yield',
}


def ident(col):
    """列名 → Rust 字段名（小写；关键字加 r#）。"""
    lo = col.lower()
    return 'r#' + lo if lo in KEYWORDS else lo


def pascal(moc):
    return ''.join(p[:1] + p[1:].lower() for p in moc.split('_'))


def load():
    sc = json.load(open(os.path.join(MGW_DATA, 'moc_schema.json'), encoding='utf-8'))
    ky = json.load(open(os.path.join(MGW_DATA, 'moc_key.json'), encoding='utf-8'))
    mocs = sc['mocs']
    keys = ky['keys']
    bitmap = ky.get('bitmap') or {}
    return mocs, keys, bitmap


def build():
    mocs, keys, bitmap = load()
    names = sorted(m for m in mocs if mocs[m]['cols'])
    lines = []
    layers = []
    bmaps = []
    defaults = []

    skipped = []
    for moc in names:
        schema = mocs[moc]
        layer = schema['layer']
        cols = schema['cols']
        if not cols:
            skipped.append(moc)          # 无列 → 不建表（参数全在 defaults / ne_config）
            continue
        ks = keys.get(moc, []) if layer == 'dedicated' else []
        ty = pascal(moc)
        head = 'mml_table! {\n    %s, %s,\n' % (moc, ty)
        if ks:
            head += '    key [%s],\n' % ', '.join(ident(k) for k in ks)
        fields = ''.join('    %s: Text,\n' % ident(c) for c in cols)
        lines.append(head + fields + '}\n')
        layers.append('0' if layer == 'dedicated' else '1')
        bmaps.append(sorted(bitmap.get(moc, [])))

    # defaults 覆盖**全部** MOC（包括不建表的那 16 个）—— 总 237 条
    for moc in sorted(mocs):
        for p, v in sorted((mocs[moc].get('defaults') or {}).items()):
            defaults.append((moc, p, v))

    out = []
    out.append('// MGW 表声明（`tools/port_mgw_schema.py` 生成，**勿手改**）。')
    out.append('//')
    out.append('// 来源：`C:/dev/ims_ws/data/mgw/moc_schema.json` + `moc_key.json`（键 / 位图）。')
    out.append('// dedicated MOC 一张表（有 key），single 层无键（`KEYED=false`，每网元一行）。')
    out.append('// 只给 **cols 非空** 的 MOC 建表：16 个 single MOC 的 cols 为空（参数全在 defaults 里），')
    out.append('// ims_core 那边也没有它们的 `_cur` 表（只进 `mgw_ne_config`）。')
    out.append('// 全部列都是 `Text`：MGW 值在 ims_core 里一律 TEXT，且原文无逗号/引号，可逐字节还原。')
    out.append('')
    out.append('use crate::mml_table;')
    out.append('use crate::table::TableApi;')
    out.append('')
    out.extend(lines)
    out.append('/// 全部 MOC（**排序**，`gen_moc_index` 用二分）。')
    out.append('pub static MGW_MOCS: &[&str] = &[')
    for n in names:
        out.append('    "%s",' % n)
    out.append('];')
    out.append('')
    out.append('/// 每个 MOC 的层：`0` = dedicated（有键）/ `1` = single（每网元一条）。')
    out.append('pub static MGW_LAYER: &[u8] = &[%s];' % ', '.join(layers))
    out.append('')
    out.append('/// 每个 MOC 里需要转"整数位掩码"的参数（`NAME-0&NAME-1` → 十进制掩码）。')
    out.append('pub static MGW_BITMAP: &[&[&str]] = &[')
    for bm in bmaps:
        out.append('    &[%s],' % ', '.join('"%s"' % x for x in bm))
    out.append('];')
    out.append('')
    out.append('/// 全局常量参数（`mgw_default`）：`(moc, param, value)`。')
    out.append('pub static MGW_DEFAULTS: &[(&str, &str, &str)] = &[')
    for moc, p, v in defaults:
        out.append('    ("%s", "%s", "%s"),' % (moc, p, v.replace('\\', '\\\\').replace('"', '\\"')))
    out.append('];')
    out.append('')
    out.append('/// MOC 名（大小写不敏感）→ 表下标。')
    out.append('pub fn gen_moc_index(name: &[u8]) -> Option<usize> {')
    out.append('    if name.is_empty() || name.len() > 16 {')
    out.append('        return None;')
    out.append('    }')
    out.append('    let mut buf = [0u8; 16];')
    out.append('    for (i, &c) in name.iter().enumerate() {')
    out.append('        buf[i] = c.to_ascii_uppercase();')
    out.append('    }')
    out.append('    MGW_MOCS.binary_search_by(|m| m.as_bytes().cmp(&buf[..name.len()])).ok()')
    out.append('}')
    out.append('')
    out.append('/// 按下标建一张空表。')
    out.append('pub fn new_gen_table(id: usize) -> Option<Box<dyn TableApi>> {')
    out.append('    Some(match id {')
    for i, moc in enumerate(names):
        out.append('        %d => Box::new(%sTable::new()),' % (i, pascal(moc)))
    out.append('        _ => return None,')
    out.append('    })')
    out.append('}')
    out.append('')

    stats = {
        'moc': len(names),
        'skipped_empty_cols': len(mocs) - len(names),
        'dedicated': sum(1 for x in layers if x == '0'),
        'single': sum(1 for x in layers if x == '1'),
        'cols': sum(len(mocs[m]['cols']) for m in names),
        'bitmap_params': sum(len(x) for x in bmaps),
        'defaults': len(defaults),
    }
    return '\n'.join(out), stats



def build_ne_map():
    """`ne_map.json` → `mgw_ne_map.tsv`（mid \t short \t name），供 core 的 `load_ne_map` 读。

    core 不引 serde，所以这里把 10 行 JSON 摊成 TSV（与 `defaults.tsv` / `zh.tsv` 同款做法）。
    """
    p = os.path.join(MGW_DATA, 'ne_map.json')
    if not os.path.exists(p):
        print('x 缺少 %s' % p, file=sys.stderr)
        return None
    d = json.load(open(p, encoding='utf-8'))
    rows = []
    for mid, v in sorted(d.items()):
        if mid.startswith('_'):
            continue
        rows.append('%s\t%s\t%s' % (mid, v.get('short', ''), v.get('name', '')))
    head = ('# MGW 网元映射（tools/port_mgw_schema.py 生成，勿手改）\n'
            '# mid(IP:port) \t short \t name   —— 来源 data/mgw/ne_map.json\n')
    return head + '\n'.join(rows) + '\n'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--write', action='store_true')
    ap.add_argument('--check', action='store_true')
    a = ap.parse_args()
    for f in ('moc_schema.json', 'moc_key.json'):
        p = os.path.join(MGW_DATA, f)
        if not os.path.exists(p):
            print('x 缺少 %s' % p, file=sys.stderr)
            return 4
    text, st = build()
    print('MGW 声明：%d MOC 建表（dedicated %d / single %d）/ %d 列 / 位图参数 %d / defaults %d；'
          '另有 %d 个 MOC 因 cols 为空不建表（参数全在 defaults）'
          % (st['moc'], st['dedicated'], st['single'], st['cols'],
             st['bitmap_params'], st['defaults'], st['skipped_empty_cols']))
    if a.check:
        if not os.path.exists(OUT):
            print('x --check：%s 不存在' % OUT, file=sys.stderr)
            return 3
        disk = open(OUT, encoding='utf-8', newline='').read().replace('\r\n', '\n')
        if disk != text:
            print('x --check：重算与磁盘不一致', file=sys.stderr)
            return 3
        print('ok --check：重算 == 磁盘')
        return 0
    if a.write:
        os.makedirs(os.path.dirname(OUT), exist_ok=True)
        with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
            f.write(text)
        print('写入 %s（%d 行）' % (OUT, text.count('\n') + 1))
        ne = build_ne_map()
        if ne is None:
            return 4
        with open(NE_OUT, 'w', encoding='utf-8', newline='\n') as f:
            f.write(ne)
        print('写入 %s' % NE_OUT)
    else:
        print('（未写文件；加 --write）')
    return 0


if __name__ == '__main__':
    sys.exit(main())
