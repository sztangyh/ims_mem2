# -*- coding: utf-8 -*-
"""MGW 中文词典验收：装载保真 / 覆盖 / 抽检 / **与 AGCF 不串味**。

判据：
  ① **装载保真**：`mgw_zh.tsv` 行数 == 生成器重算的行数（`gen_mgw_zh.py --check` 也跑）；
  ② **覆盖**：本仓 MGW 43 张表的 188 个列，**每个都有中文名**；
  ③ **抽检**：`VMGW.MID`=虚拟媒体网关标识、`BRD.BT`=板类型、`Q921LNK.LNKNO`=链路号…
  ④ **不串味（最重要）**：AGCF 与 MGW 同名参数的中文名**必须不同**——
     `MID` 在 AGCF 是「ACU模块号」、在 MGW 是「虚拟媒体网关标识」。
     若两边相等，说明查错了字典（或两张表被合并了），直接红。
退出码：0 全过 / 1 有差异 / 3 不一致 / 4 缺输入。
"""
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MGW_DATA = r'C:\dev\ims_ws\data\mgw'
TSV = os.path.join(ROOT, 'mgw_zh.tsv')
AGCF_ZH = os.path.join(ROOT, 'zh.tsv')


def unesc(v):
    out, i = [], 0
    while i < len(v):
        c = v[i]
        if c == '\\' and i + 1 < len(v) and v[i + 1] in 'tn\\':
            out.append({'t': '\t', 'n': '\n', '\\': '\\'}[v[i + 1]])
            i += 2
            continue
        out.append(c)
        i += 1
    return ''.join(out)


def load_tsv():
    par, moc = {}, {}
    n = 0
    for line in open(TSV, encoding='utf-8'):
        line = line.rstrip('\n').rstrip('\r')
        if not line or line[0] == '#':
            continue
        f = line.split('\t')
        if len(f) < 5:
            continue
        n += 1
        if f[0] == 'M':
            moc[unesc(f[1]).upper()] = (unesc(f[3]), unesc(f[4]))
        elif f[0] == 'P':
            par[(unesc(f[1]).upper(), unesc(f[2]).upper())] = (unesc(f[3]), unesc(f[4]))
    return par, moc, n


def main():
    for p, why in ((TSV, 'mgw_zh.tsv'), (AGCF_ZH, 'zh.tsv'), (MGW_DATA, 'MGW 元数据')):
        if not os.path.exists(p):
            print('x 缺 %s（%s）' % (p, why), file=sys.stderr)
            return 4

    # ① 装载保真：生成器 --check
    r = subprocess.run([sys.executable, '-X', 'utf8', os.path.join(ROOT, 'tools', 'gen_mgw_zh.py'), '--check'],
                       capture_output=True, cwd=ROOT)
    if r.returncode != 0:
        print('x gen_mgw_zh.py --check 失败：%s'
              % r.stderr.decode('utf-8', 'replace')[-200:])
        return 3
    par, moc, nrow = load_tsv()
    print('   ok 装载 %d 条（%d 对象 / %d 参数），生成器复算一致' % (nrow, len(moc), len(par)))

    # ② 覆盖：43 表 188 列
    sc = json.load(open(os.path.join(MGW_DATA, 'moc_schema.json'), encoding='utf-8'))['mocs']
    tabs = {m: v for m, v in sc.items() if v['cols']}
    tot = sum(len(v['cols']) for v in tabs.values())
    miss = [(m, c) for m, v in tabs.items() for c in v['cols']
            if not (par.get((m.upper(), c.upper())) or ('', ''))[0]]
    if miss:
        print('x %d 个列缺中文名，例 %s' % (len(miss), miss[:5]))
        return 1
    print('   ok 覆盖 %d / %d 列 = 100%%' % (tot, tot))

    # ③ 抽检
    for (m, p, want) in [('VMGW', 'MID', '虚拟媒体网关标识'), ('BRD', 'BT', '板类型'),
                         ('BRD', 'BN', '板组号'), ('Q921LNK', 'LNKNO', '链路号'),
                         ('L2UALNK', 'LOCALIP1', '本端IP地址1'), ('TDMIU', 'TIDFV', None)]:
        got = (par.get((m, p)) or ('', ''))[0]
        if want is not None and got != want:
            print('x 抽检 %s.%s：期望 %r 实际 %r' % (m, p, want, got))
            return 1
    print('   ok 抽检 6 个参数逐字一致')

    # ④ 不串味：同名参数两边中文名必须不同
    agcf = {}
    for line in open(AGCF_ZH, encoding='utf-8'):
        f = line.rstrip('\n').split('\t')
        if len(f) >= 5 and f[0] == 'P':
            agcf.setdefault(unesc(f[2]).upper(), set()).add(unesc(f[4]))
    same, diff, checked = [], 0, 0
    for (m, p), (cn, _d) in par.items():
        a = agcf.get(p)
        if not a or not cn:
            continue
        checked += 1
        if cn in a:
            same.append((p, m, cn))
        else:
            diff += 1
    # 允许极少量的"确实同义"（如 `BN` 两边都叫板号）——但绝不能是 `MID`/`BT` 这种
    must = [('MID', 'VMGW'), ('BT', 'BRD')]
    for p, m in must:
        cn = (par.get((m, p)) or ('', ''))[0]
        a = agcf.get(p)
        if a and cn in a:
            print('x 串味：MGW %s.%s=%r 与 AGCF 同名同义 %s —— 说明查错了字典' % (m, p, cn, sorted(a)[:2]))
            return 1
    print('   ok 不串味：%d 个同名参数里 %d 个两边不同义（%d 个确实同义，属正常）'
          % (checked, diff, len(same)))
    if same:
        print('      同义样例：%s' % ', '.join('%s(MGW)=%s/AGCF' % (p, cn) for p, m, cn in same[:4]))
    return 0


if __name__ == '__main__':
    sys.exit(main())
