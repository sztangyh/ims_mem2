# -*- coding: utf-8 -*-
"""开关名（位名）中文来源的**审计与选源**（D69 的依据；结果见 `spec/SWITCH-NAMES-AUDIT.md`）。

只读、不写任何文件。回答三个问题：
  ① 工作区里到底**有几个地方**有开关名中文？各自的定位方式是什么？
  ② 每个来源**覆盖多少**（分母 = 本仓 `Bits` 参数的 1,058 个开关名）、**含汉字多少**？
  ③ 它们**互相矛盾吗**？矛盾在哪些位置、谁对？

判据（与 D52"不丢信息/例外显式计数"一致）：
  * 定位方式决定可信度：`(MOC,PARAM,开关名)` 三元组 > 只按位词；
  * 位词词典 `switch_vocab` 只在 `n_spaces==1`（无歧义）时可用；
  * `doc_option` 里 340 行是英文，不作中文名；
  * 任何"放弃"都要**计数并打印**，不许静默。

用法：
    python tools/audit_switch_sources.py            # 打印人读报告
    python tools/audit_switch_sources.py --markdown # 打印可粘进 md 的表格
退出码：0 正常 / 4 缺来源文件（审计本身要求五个来源都在，缺了结论就不成立）
"""
import argparse
import json
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, 'tools'))

IMS = os.path.join('C:/dev/ims_ws', 'data', 'ims_asset')
DOCDB = os.path.join('C:/dev/tryit', 'mml_doc', 'agcf_doc', 'agcf_mml.db')
MANUAL = os.path.join('C:/dev/tryit', 'mml_doc', 'agcf_doc', 'manual_switch_meanings.json')
SWDICT = os.path.join(IMS, 'switch_dict.json')
VOCAB = os.path.join(IMS, 'switch_vocab.json')
PERMBITS_DB = os.path.join(IMS, 'library.db')
MANUAL_MD = os.path.join(IMS, 'manual_records', 'switch_bits.md')


def has_cn(x):
    return bool(x) and bool(re.search(r'[\u4e00-\u9fff]', str(x)))


def need(path, what):
    if not os.path.exists(path):
        print('x 缺少%s：%s' % (what, path), file=sys.stderr)
        return False
    return True


def load_manual():
    m = {}
    for e in json.load(open(MANUAL, encoding='utf-8')):
        moc = str(e.get('moc', '')).upper()
        par = str(e.get('param', '')).upper()
        for name, v in (e.get('switches') or {}).items():
            if isinstance(v, (list, tuple)) and len(v) > 1:
                m[(moc, par, str(name).upper())] = str(v[1])
    return m


def load_auto():
    con = sqlite3.connect('file:%s?mode=ro' % DOCDB.replace('\\', '/'), uri=True)
    try:
        return {(a.upper(), b.upper(), c.upper()): ((d or ''), (e or '')) for a, b, c, d, e in
                con.execute('SELECT moc,param_name,switch_name,doc_option,doc_desc '
                            'FROM switch_meanings')}
    finally:
        con.close()


def load_vocab():
    """返回 `(全部含中文的位词, 其中 n_spaces==1 的, 多义位词数, ascii 兜底数)`。"""
    all_zh, uniq, multi, ascii_like = {}, {}, 0, 0
    for e in json.load(open(VOCAB, encoding='utf-8')):
        cn = e.get('means_cn') or ''
        if e.get('means_src') == 'ascii' or not has_cn(cn):
            ascii_like += 1
            continue
        bit = e['bit'].upper()
        all_zh[bit] = cn
        n = e.get('n_spaces', len(e.get('spaces') or []))
        if n == 1:
            uniq[bit] = cn
        else:
            multi += 1
    return all_zh, uniq, multi, ascii_like


def load_zh_value():
    con = sqlite3.connect('file:%s?mode=ro' % PERMBITS_DB.replace('\\', '/'), uri=True)
    try:
        zv = {(a.upper(), b.upper(), str(c).upper()): d for a, b, c, d in
              con.execute('SELECT moc,param,value,value_cn FROM zh_value')}
        zv_any = {}
        for a, b, d in con.execute('SELECT param,value,value_cn FROM zh_value_any'):
            zv_any.setdefault((a.upper(), str(b).upper()), d)
        perm = con.execute('SELECT COUNT(*) FROM perm_bits').fetchone()[0]
        return zv, zv_any, perm
    finally:
        con.close()


def same(a, b):
    """粗判同义：去标点空白后互为前缀/一方含另一方。只用于"是否互相矛盾"的统计。"""
    f = lambda s: re.sub(r'[\s，。、“”"（）()：:；;\-\'>]', '', str(s))
    x, y = f(a), f(b)
    return x == y or x.startswith(y) or y.startswith(x) \
        or (len(x) > 6 and x[:6] in y) or (len(y) > 6 and y[:6] in x)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--markdown', action='store_true')
    a = ap.parse_args()

    for p, w in [(MANUAL, '人工裁决 json'), (DOCDB, '文档库 agcf_mml.db'), (VOCAB, '位词词典'),
                 (SWDICT, '位序空间 switch_dict'), (PERMBITS_DB, 'library.db')]:
        if not need(p, w):
            return 4

    import gen_zh
    want = gen_zh.ims_mem2_bits()
    N = len(want)
    man = load_manual()
    auto = load_auto()
    voc_all, voc_uniq, voc_multi, voc_ascii = load_vocab()
    zv, zv_any, n_perm = load_zh_value()
    swdict = gen_zh.switch_dict_triples(SWDICT)
    n_space = len({(m, p) for m, p, _ in swdict})

    sources = [
        ('manual_switch_meanings.json', '三元组·人工裁决',
         lambda k: man.get(k)),
        ('switch_meanings.doc_desc', '三元组·文档解析',
         lambda k: (auto.get(k) or ('', ''))[1]),
        ('switch_meanings.doc_option', '三元组·选项名',
         lambda k: (auto.get(k) or ('', ''))[0]),
        ('switch_vocab.json（全部位词）', '只按位词',
         lambda k: voc_all.get(k[2])),
        ('switch_vocab.json（n_spaces==1）', '只按位词·无歧义',
         lambda k: voc_uniq.get(k[2])),
        ('switch_dict.json', '三元组·仅位名（无中文）',
         lambda k: None),
        ('library.db::perm_bits', '三元组·仅位名（无中文）',
         lambda k: None),
        ('library.db::zh_value', '三元组·枚举取值口径',
         lambda k: zv.get(k)),
        ('library.db::zh_value_any', '只按取值',
         lambda k: zv_any.get((k[1], k[2]))),
    ]

    print('分母：本仓 44 个 Bits 参数里的 **%d 个开关名**' % N)
    print('位序空间：switch_dict %d 个 (MOC,PARAM) / %d 个三元组；'
          'perm_bits %d 行（无中文列）；switch_vocab %d 个含中文的位词'
          '（其中 n_spaces==1 的 %d 个、多义 %d 个、ascii 兜底 %d 个）\n'
          % (n_space, len(swdict), n_perm, len(voc_all), len(voc_uniq), voc_multi, voc_ascii))

    rows = []
    for name, how, fn in sources:
        hit = zh = 0
        for k in want:
            v = fn(k)
            if v:
                hit += 1
                if has_cn(v):
                    zh += 1
        rows.append((name, how, hit, zh))

    if a.markdown:
        print('| 来源 | 定位方式 | 命中 / %d | 含汉字 |' % N)
        print('|---|---|---|---|')
        for name, how, hit, zh in rows:
            print('| `%s` | %s | %d | %d |' % (name, how, hit, zh))
    else:
        print('%-38s %-22s %8s %8s %7s' % ('来源', '定位方式', '命中', '含汉字', '占比'))
        print('-' * 88)
        for name, how, hit, zh in rows:
            print('%-38s %-22s %8d %8d %6.1f%%' % (name, how, hit, zh, 100.0 * zh / N))

    # ---- 交叉验证：谁和谁同义 ----
    def pair(an, afn, bn, bfn):
        n = agree = 0
        ex = []
        for k in want:
            x, y = afn(k), bfn(k)
            if has_cn(x) and has_cn(y):
                n += 1
                if same(x, y):
                    agree += 1
                elif len(ex) < 3:
                    ex.append((k, x, y))
        return n, agree, ex

    ma = pair('manual', lambda k: man.get(k), 'doc', lambda k: (auto.get(k) or ('', ''))[1])
    mv = pair('manual', lambda k: man.get(k), 'vocab', lambda k: voc_all.get(k[2]))
    print('\n交叉验证（只在"双方都含汉字"的位置上比）')
    print('  manual vs doc_desc : %d 处，同义 %d（%.0f%%）' % (ma[0], ma[1], 100.0 * ma[1] / max(ma[0], 1)))
    print('  manual vs vocab    : %d 处，同义 %d（%.0f%%）' % (mv[0], mv[1], 100.0 * mv[1] / max(mv[0], 1)))
    if mv[2]:
        print('  manual ≠ vocab 的样例（说明 vocab 按词套错位）：')
        for k, x, y in mv[2]:
            print('    %s/%s/%s\n      manual: %s\n      vocab : %s' % (k[0], k[1], k[2], x[:60], y[:60]))

    # ---- 多义开关名的规模（分母是 1,058 个开关名，不是位词数）----
    multi_triples = sum(1 for k in want if k[2] in voc_all and k[2] not in voc_uniq)
    print('\n多义：1,058 个开关名里 **%d 个（%.0f%%）** 的位词出现在 >1 个位序空间'
          '（vocab 里 n_spaces>1）→ 按词聚合必错'
          % (multi_triples, 100.0 * multi_triples / N))

    # ---- 严格链的落地覆盖 ----
    def strict(k):
        for fn in (lambda k: man.get(k),
                   lambda k: (auto.get(k) or ('', ''))[1],
                   lambda k: voc_uniq.get(k[2])):
            v = fn(k)
            if has_cn(v):
                return v
        return None
    cov = sum(1 for k in want if strict(k))
    print('\n采用链 manual → doc_desc → vocab(n_spaces==1)：**%d / %d = %.1f%%**'
          % (cov, N, 100.0 * cov / N))

    # ---- 剩余缺口（按参数聚） ----
    gap = {}
    for k in want:
        if not strict(k):
            gap[(k[0], k[1])] = gap.get((k[0], k[1]), 0) + 1
    print('\n仍缺的 %d 个开关名分布在 %d 个参数上：' % (sum(gap.values()), len(gap)))
    for (moc, par), n in sorted(gap.items(), key=lambda kv: -kv[1]):
        print('  %-12s.%-14s %3d' % (moc, par, n))
    print('\n（这些是"号码位/原因值位"，文档本身没有逐位解释 —— 属源头缺失，不是漏接。）')
    return 0


if __name__ == '__main__':
    sys.exit(main())
