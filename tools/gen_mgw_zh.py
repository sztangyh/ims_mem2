# -*- coding: utf-8 -*-
"""MGW 中文词典：`ims_ws/data/mgw/moc_zh.json` → 仓内 `mgw_zh.tsv`（冻结快照）。

**为什么单独一份、不并进 `zh.tsv`**：AGCF 与 MGW 的参数**同名不同义**——`MID` 在 AGCF 是
「ACU模块号」、在 MGW 是「虚拟媒体网关标识」，`BT`/`BN` 之类的短名更是到处复用。
并成一份就必须靠 `moc` 区分，而两边的 `moc` 也有 7 个同名（BRD/IPADDR/N7DSP/N7LKS/N7LNK/N7RT/OFI），
查询时极易串味。分开存、分开查，从结构上杜绝。

格式（`#` 注释 + 4 列）：

    # kind \t moc \t param \t cn \t desc
    M\tATONE\t\t增加异步音\t增加异步音(ADD ATONE)
    P\tATONE\tBT\t板类型\t必选参数。该参数为枚举类型…

用法：
    python tools/gen_mgw_zh.py             # 只算 + 报覆盖
    python tools/gen_mgw_zh.py --write     # 写 mgw_zh.tsv
    python tools/gen_mgw_zh.py --check     # 重算 == 磁盘（不一致退出码 3）
退出码：0 成功 / 3 不一致 / 4 缺输入。
"""
import argparse
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MGW_DATA = os.path.join('C:/dev/ims_ws', 'data', 'mgw')
OUT = os.path.join(ROOT, 'mgw_zh.tsv')


def esc(v):
    if v is None:
        return ''
    return (str(v).replace('\\', '\\\\').replace('\t', '\\t')
            .replace('\r', '').replace('\n', '\\n'))


def unesc(v):
    out, i = [], 0
    while i < len(v):
        c = v[i]
        if c == '\\' and i + 1 < len(v):
            n = v[i + 1]
            if n == 't':
                out.append('\t'); i += 2; continue
            if n == 'n':
                out.append('\n'); i += 2; continue
            if n == '\\':
                out.append('\\'); i += 2; continue
        out.append(c); i += 1
    return ''.join(out)


def build():
    p = os.path.join(MGW_DATA, 'moc_zh.json')
    if not os.path.exists(p):
        print('x 缺少 %s' % p, file=sys.stderr)
        return None
    d = json.load(open(p, encoding='utf-8'))['mocs']
    sc = json.load(open(os.path.join(MGW_DATA, 'moc_schema.json'), encoding='utf-8'))['mocs']
    rows = []
    n_moc = n_par = n_desc = 0
    for moc in sorted(d):
        v = d[moc]
        if v.get('cn') or v.get('title'):
            rows.append('M\t%s\t\t%s\t%s' % (esc(moc), esc(v.get('cn') or ''), esc(v.get('title') or '')))
            n_moc += 1
        for par in sorted((v.get('params') or {})):
            e = v['params'][par]
            if not (e.get('cn') or e.get('desc')):
                continue
            rows.append('P\t%s\t%s\t%s\t%s' % (esc(moc), esc(par), esc(e.get('cn') or ''),
                                               esc(e.get('desc') or '')))
            n_par += 1
            if e.get('desc'):
                n_desc += 1
    # 对本仓 43 张表 / 188 列的覆盖（这些是"必须都有中文名"的部分）
    tabs = {m: x for m, x in sc.items() if x['cols']}
    tot = sum(len(x['cols']) for x in tabs.values())
    pmap = {(m.upper(), p.upper()) for m, v in d.items() for p in (v.get('params') or {})}
    have = sum(1 for m, x in tabs.items() for c in x['cols'] if (m.upper(), c.upper()) in pmap)
    head = ('# MGW 中文词典（tools/gen_mgw_zh.py 生成，勿手改）\n'
            '# 来源：ims_ws/data/mgw/moc_zh.json（59 对象 / 712 参数，来自 umg_doc）\n'
            '# kind \t moc \t param \t cn \t desc   （M=对象，P=参数）\n')
    text = head + '\n'.join(rows) + '\n'
    stats = {'moc': n_moc, 'param': n_par, 'desc': n_desc,
             'cols_have': have, 'cols_tot': tot}
    return text, stats


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--write', action='store_true')
    ap.add_argument('--check', action='store_true')
    a = ap.parse_args()
    got = build()
    if got is None:
        return 4
    text, st = got
    print('MGW 中文：对象 %d / 参数 %d（有说明 %d）；对 43 表 %d 列覆盖 %d = %.1f%%'
          % (st['moc'], st['param'], st['desc'], st['cols_tot'], st['cols_have'],
             100.0 * st['cols_have'] / st['cols_tot']))
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
        with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
            f.write(text)
        print('写入 %s（%d 行 / %.0f KB）'
              % (OUT, text.count('\n') + 1, len(text.encode('utf-8')) / 1024.0))
    return 0


if __name__ == '__main__':
    sys.exit(main())
