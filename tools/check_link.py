# -*- coding: utf-8 -*-
"""M4 验收（第一切片）：7 条 IO→IO 单跳边，对 33 个金标准独立重建的 oracle 比对。

每条边比三个数：`source_rows` / `resolved` / `dangling`。
oracle 用同一套规则从金标准 NDJSON 算（键 = (ne_id, 目标键) 是否存在）。
"""
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = r'C:\dev\tryit\.temp'
EXE = os.path.join(ROOT, 'target', 'release', 'examples', 'io_link.exe')
EXEMPT = {'cur_from', 'diff'}


def load(name):
    p = os.path.join(GOLDEN, f'bridge_{name}.ndjson')
    with open(p, encoding='utf-8') as f:
        return [json.loads(l) for l in f if l.strip()]


def keyset(rows, col):
    s = set()
    for r in rows:
        v = r.get(col)
        if v is not None and str(v) != '':
            s.add((r['ne_id'], str(v)))
    return s


def stat(src, keyfn, target):
    n = res = 0
    for r in src:
        for k in keyfn(r):
            n += 1
            if (r['ne_id'], k) in target:
                res += 1
    return n, res, n - res


def oracle():
    trunk = set()
    for name, col in (('pratg', 'tg'), ('sip_tg', 'tg'), ('n7tg', 'tg')):
        trunk |= keyset(load(name), col)
    rt = keyset(load('rt'), 'r')
    srt = keyset(load('srt'), 'src')
    srtattr = keyset(load('srtattr'), 'sr')
    v5i = keyset(load('v5i'), 'v5iid')

    asbr_trunk = load('asbr_trunk')
    asbr_v5 = load('asbr_v5')
    pratg = load('pratg')
    tgsrt = load('tgsrt')
    rtana = load('rtana')
    rtrows = load('rt')
    srtrows = load('srt')

    def sr_of(r):
        out = []
        for i in range(1, 29):
            v = r.get(f'sr{i}')
            if v is not None and str(v) not in ('', '0'):
                out.append(str(v))
        return out

    return {
        'Subscriber.trunk': stat(asbr_trunk, lambda r: [str(r['tg'])] if r.get('tg') else [], trunk),
        'Subscriber.v5i': stat(asbr_v5, lambda r: [str(r['v5iid'])] if r.get('v5iid') else [], v5i),
        'PraLink.trunk': stat(pratg, lambda r: [str(r['tg'])] if r.get('tg') else [], trunk),
        'Tgsrt.trunk': stat(tgsrt, lambda r: [str(r['tg'])] if r.get('tg') else [], trunk),
        'Rtana.rt': stat(rtana, lambda r: [str(r['r'])] if r.get('r') else [], rt),
        'Rt.srt': stat(rtrows, sr_of, srt),
        'Srt.attr': stat(srtrows, lambda r: [str(r['src'])] if r.get('src') else [], srtattr),
    }


# 已登记例外：ours 与 oracle 允许的偏差（金标准多算的部分）
ALLOW = {
    # PRATG 折叠表 4,376 行；金标准 pratg 4,378 行（多 2 条 PRALNK 孤儿，D53 在案）
    'PraLink.trunk': 2,
}


def main():
    want = oracle()
    files = [os.path.join(r'C:\dev\tryit\agcf_data', x)
             for x in sorted(os.listdir(r'C:\dev\tryit\agcf_data'))
             if x.startswith('ALLME_2026') and x.endswith('.txt')]
    p = subprocess.run([EXE, *files], capture_output=True, text=True)
    if p.returncode != 0:
        print('io_link 失败:', p.stderr[-300:])
        return 1
    got = json.loads(p.stdout)

    fails = 0
    print(f'{"边":20} {"source":>8} {"resolved":>9} {"dangling":>9}  结果')
    for edge in sorted(want):
        w = want[edge]
        g = got.get(edge, {'source_rows': 0, 'resolved': 0, 'dangling': 0})
        allow = ALLOW.get(edge, 0)
        ok = (w[0] - g['source_rows'] == allow and w[1] - g['resolved'] == allow
              and w[2] == g['dangling'])
        print(f'{edge:20} {g["source_rows"]:>8} {g["resolved"]:>9} {g["dangling"]:>9}  '
              f'{"ok" if ok else f"x oracle={w}"}')
        if not ok:
            fails += 1
    if fails:
        print(f'\nx M4 链接校验失败 {fails} 条边')
        return 1
    print('\nok M4 链接（7 条单跳边）与 oracle 一致')
    return 0


if __name__ == '__main__':
    sys.exit(main())
