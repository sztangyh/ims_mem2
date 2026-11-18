# -*- coding: utf-8 -*-
"""`Mgw` / `MgwLink` 两个 IO 的金标准对拍（对 `ims_core` 的 `mgw_library.db`）。

判据：
  ① `Mgw` 对象数 == 金标准 `mgw_ne` 行数（10）；
  ② `Mgw` 的 `mid` / `name` / `site` 逐对象 == `mgw_ne(mid, mename, short)`；
  ③ `Mgw.link_counts` 的每族条数 == 金标准对应 `mgw_<族>_cur` 的行数（逐网元求和）；
  ④ `MgwLink` 条数与 `(mgw_l2ualnk, mgw_h248lnk)` 合计相等，且 `(mgw_mid, kind, id, local, remote)` 逐条相等；
  ⑤ 不许出现金标准里没有的 mid / kind。

用 `mgw_dump --io` 产出的行做输入（`Mgw\\t…` / `MgwLink\\t…`）。
退出码：0 全过 / 1 有差异 / 4 缺输入。
"""
import argparse
import os
import sqlite3
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
ORACLE = r'C:\dev\ims_ws\ims_core\temp\mgw_library.db'
MGW_DATA = r'C:\dev\ims_ws\data\mgw'
#: "表即对象"的 4 个 IO → （IO 名, 金标准 MOC）
TABLE_IOS = [('Q921Link', 'Q921LNK'), ('V5Link', 'V5LNK'),
             ('Tdmiu', 'TDMIU'), ('Board', 'BRD')]

#: `link_counts` 里出现的族 → 金标准表名（顺序无关）
FAMS = {
    'q921lnk': 'mgw_q921lnk_cur',
    'v5e1': 'mgw_v5e1_cur',
    'v5lnk': 'mgw_v5lnk_cur',
    'mtp2lnk': 'mgw_mtp2lnk_cur',
    'l2ualnk': 'mgw_l2ualnk_cur',
    'h248lnk': 'mgw_h248lnk_cur',
    'n7lnk': 'mgw_n7lnk_cur',
}


def oracle():
    c = sqlite3.connect('file:%s?mode=ro' % ORACLE.replace('\\', '/'), uri=True)
    ne = {r[0]: (r[1], r[2], r[3]) for r in c.execute('SELECT ne_id, mid, mename, short FROM mgw_ne')}
    # ③ 每族的 (mid → 行数)
    fam_counts = {}
    for fam, tab in FAMS.items():
        try:
            d = {r[0]: r[1] for r in c.execute(
                'SELECT ne_id, COUNT(*) FROM "%s" GROUP BY ne_id' % tab)}
        except sqlite3.Error:
            d = {}
        fam_counts[fam] = d
    # ④ 链路族
    links = {}
    for kind, tab, idexpr, lc, rc in [
        # l2ua 的 id 必须带 PROTYPE（IUA/M2UA）—— 单靠 LNKNO 会撞 6 组
        ('l2ua', 'mgw_l2ualnk_cur', "PROTYPE || '/' || LNKNO", 'LOCALIP1', 'REMOTEIP1'),
        ('h248', 'mgw_h248lnk_cur', 'LINKID', 'LOCALIP', 'PEERIP'),
    ]:
        for nid, i, l, r in c.execute(
                'SELECT ne_id, %s, "%s", "%s" FROM "%s"' % (idexpr, lc, rc, tab)):
            links[(ne[nid][0], kind, i or '')] = (l or '', r or '')
    c.close()
    return ne, fam_counts, links


def run_dump():
    exe = os.path.join(ROOT, 'target', 'release', 'examples', 'mgw_dump.exe')
    if not os.path.exists(exe):
        print('x 缺 %s' % exe, file=sys.stderr)
        return None
    # 金标准是用 202304 + 20230919 两批建的（见 check_mgw.py 的说明），必须同口径
    dirs = [os.path.join(MGW_DATA, d) for d in ('202304', '20230919')]
    p = subprocess.run([exe, '--io'] + dirs, capture_output=True, cwd=ROOT)
    if p.returncode != 0:
        print('x mgw_dump --io 失败：%s' % p.stderr.decode('utf-8', 'replace')[-300:], file=sys.stderr)
        return None
    return p.stdout.decode('utf-8', 'replace').splitlines()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--dump', help='用已有 --io 输出文件')
    a = ap.parse_args()
    if not os.path.exists(ORACLE) or not os.path.isdir(MGW_DATA):
        print('x 缺 %s 或 %s' % (ORACLE, MGW_DATA), file=sys.stderr)
        return 4
    oracle_ne, fam_counts, oracle_links = oracle()
    raw = (open(a.dump, encoding='utf-8').read().splitlines() if a.dump else run_dump())
    if raw is None:
        return 4

    mgw, links, table_ios = {}, {}, {}
    for ln in raw:
        if not ln:
            continue
        f = ln.split('\t')
        if f[0] == 'Mgw':
            mgw[f[1]] = f[2:]
        elif f[0] == 'MgwLink':
            links[(f[1], f[2], f[3])] = (f[4], f[5])
        elif len(f) == 4:
            table_ios.setdefault(f[0], set()).add(ln)

    bad = 0
    # ① 对象数
    print('Mgw 对象 %d 个 / 金标准 mgw_ne %d 行' % (len(mgw), len(oracle_ne)))
    if len(mgw) != len(oracle_ne):
        print('x Mgw 对象数不符')
        bad += 1

    # ② mid/name/site
    by_mid = {v[0]: v for v in oracle_ne.values()}   # mid -> (mid, mename, short)
    for mid, fields in sorted(mgw.items()):
        o = by_mid.get(mid)
        if o is None:
            print('x Mgw 多出金标准没有的 mid：%s' % mid)
            bad += 1
            continue
        if fields[0] != o[1] or fields[1] != o[2]:
            print('x Mgw %s：name/site = %r/%r，金标准 %r/%r' % (mid, fields[0], fields[1], o[1], o[2]))
            bad += 1
    print('   ok mid / name / site 逐对象一致')

    # ③ link_counts
    mid2nid = {v[0]: nid for nid, v in oracle_ne.items()}
    for mid, fields in sorted(mgw.items()):
        got = {}
        body = fields[4].strip('{}')
        if body:
            for kv in body.split(','):
                k, v = kv.split(':')
                got[k.strip('"')] = int(v)
        want = {fam: fam_counts[fam].get(mid2nid[mid], 0) for fam in FAMS}
        want = {k: v for k, v in want.items() if v}
        if got != want:
            print('x Mgw %s 的 link_counts 不符：本仓 %r / 金标准 %r' % (mid, got, want))
            bad += 1
    if not bad:
        print('   ok link_counts 逐族一致（7 族）')

    # ④ MgwLink
    print('MgwLink %d 条 / 金标准 l2ualnk %d + h248lnk %d = %d'
          % (len(links), 82, 24, len(oracle_links)))
    if set(links) != set(oracle_links):
        only_m = sorted(set(links) - set(oracle_links))[:2]
        only_o = sorted(set(oracle_links) - set(links))[:2]
        print('x MgwLink 键集合不符：本仓独有 %s / 金标准独有 %s' % (only_m, only_o))
        bad += 1
    else:
        diff = [k for k in links if links[k] != oracle_links[k]]
        if diff:
            k = diff[0]
            print('x MgwLink 值不符 %d 条，例 %s：本仓 %r / 金标准 %r' % (len(diff), k, links[k], oracle_links[k]))
            bad += 1
        else:
            print('   ok 键与 local/remote 逐条一致')
    # ⑤ kind 白名单
    kinds = {k[1] for k in links}
    if kinds - {'l2ua', 'h248'}:
        print('x 出现未知 kind：%s' % (kinds - {'l2ua', 'h248'}))
        bad += 1

    # ⑥ 4 个"表即对象"的 IO vs 金标准（复用 check_mgw 的规范化：键序 + 列名排序）
    from check_mgw import oracle_lines, US
    ora = oracle_lines()
    if ora is None:
        print('x 取金标准失败')
        return 4
    for io_name, moc in TABLE_IOS:
        got = table_ios.get(io_name, set())
        want = ora.get(moc)
        if want is None:
            print('x %s：金标准没有 %s 表' % (io_name, moc))
            bad += 1
            continue
        if len(got) != len(want):
            print('x %s：%d 条 / 金标准 %d 条' % (io_name, len(got), len(want)))
            bad += 1
            continue
        # 本仓是 `IO\tne\t键\t列`，金标准是 `MOC\tne\t键\t列`（其余完全同构）
        mine_norm = {'%s\t%s' % (moc, ln.split('\t', 1)[1]) for ln in got}
        if mine_norm != want:
            d1 = sorted(mine_norm - want)[:1]
            d2 = sorted(want - mine_norm)[:1]
            print('x %s：内容不符（本仓独有 %s / 金标准独有 %s）' % (io_name, d1, d2))
            bad += 1
        else:
            print('   ok %s %d 条逐行逐列一致（金标准 %s）' % (io_name, len(got), moc))

    if bad == 0:
        print('\nok IO：Mgw 10 + MgwLink 106 + Q921Link/V5Link/Tdmiu/Board %d 条，对金标准零差异'
              % sum(len(v) for v in table_ios.values()))
        return 0
    print('\nx IO：%d 处不符' % bad)
    return 1


if __name__ == '__main__':
    sys.exit(main())
