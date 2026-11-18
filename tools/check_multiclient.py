# -*- coding: utf-8 -*-
"""同 IP 多客户端 + 默认 UDP + env 回退验收。

**自给自足**：自己建 AGCF 数据夹具（junction），并校验 svc 真的导入了 6 个文件 ——
不然夹具缺失会表现成"items=0"，看起来像产品故障（这次就被它坑了一轮）。
"""
import http.client
import json
import os
import subprocess
import sys
import time

ROOT = r'C:\dev\tryit\ims_mem2'
SVC, W1, W2, W3 = 7891, 7892, 7893, 7894
PSK = 'aa' * 32
LINK = os.path.join(ROOT, 'temp', 'agcf')
DATA_GLOB = os.path.join(LINK, 'ALLME_2026*.txt')
SVC_LOG = os.path.join(ROOT, 'temp', 'mc_svc.log')


def ensure_data():
    """建 junction 指向 ..\\agcf_data（已存在就复用）。"""
    src = os.path.join(ROOT, '..', 'agcf_data')
    if not os.path.isdir(src):
        print('x 找不到 AGCF 数据源 %s' % src, file=sys.stderr)
        return False
    if not os.path.exists(LINK):
        subprocess.run(['cmd', '/c', 'mklink', '/J', LINK, src], capture_output=True)
    return os.path.isdir(LINK)


def wait_port_ok(port, timeout=45):
    end = time.time() + timeout
    while time.time() < end:
        try:
            c = http.client.HTTPConnection('127.0.0.1', port, timeout=3)
            c.request('GET', '/api/hello')
            b = c.getresponse().read()
            c.close()
            if b'ok":true' in b.replace(b' ', b''):
                return True
        except Exception:
            pass
        time.sleep(0.4)
    return False


def q(port, path):
    c = http.client.HTTPConnection('127.0.0.1', port, timeout=90)
    c.request('GET', path)
    b = c.getresponse().read()
    c.close()
    return b


def start_web(port, extra=None, tag='w'):
    f = open(os.path.join(ROOT, 'temp', 'mc_%s.log' % tag), 'w')
    env = dict(os.environ, IMS_PSK=PSK, IMS_DAT_SVC='127.0.0.1:%d' % SVC)
    args = [os.path.join(ROOT, 'target', 'release', 'ims_web.exe')] + (extra or []) + \
           ['--http', '127.0.0.1:%d' % port]
    return subprocess.Popen(args, cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=f)


def main():
    if not ensure_data():
        return 4
    procs = []
    try:
        env = dict(os.environ, IMS_PSK=PSK, IMS_DAT_SVC='127.0.0.1:%d' % SVC)
        f = open(SVC_LOG, 'w')
        svc = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_svc.exe'),
                                '--udp', '--mgw', r'C:\dev\ims_ws\data\mgw', DATA_GLOB],
                               cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=f)
        procs.append(svc)
        time.sleep(16)
        log = open(SVC_LOG, encoding='utf-8', errors='replace').read()
        imp = [l for l in log.splitlines() if '导入完成' in l]
        print('svc 导入: %s' % (imp[0].strip() if imp else '(无)'))
        if not imp or '6 个文件' not in imp[0]:
            print('x 夹具没生效（期望 6 个文件）—— 后续断言无意义', file=sys.stderr)
            return 4

        w1 = start_web(W1, ['--svc', '127.0.0.1:%d' % SVC], 'w1')   # 需求 2：不带 scheme
        procs.append(w1)
        w2 = start_web(W2, [], 'w2')                                # 需求 3：只靠 env
        procs.append(w2)
        w3 = start_web(W3, ['--svc', 'udp://127.0.0.1:%d' % SVC], 'w3')
        procs.append(w3)

        ok2 = wait_port_ok(W1)
        ok3 = wait_port_ok(W2)
        ok4 = wait_port_ok(W3)
        print('三个实例起来: %s / %s / %s' % (ok2, ok3, ok4))

        big = '/api/catalog?pool=prefix&kind=prefix&arg=1&limit=500'
        allok = True
        print('\n--- 同 IP 三客户端并发（各 3 次 76 KB）：---')
        for rnd in range(3):
            row = []
            for port in (W1, W2, W3):
                b = q(port, big)
                try:
                    n = len(json.loads(b).get('items') or [])
                except Exception:
                    n = -1
                row.append(n)
                allok &= n == 500
            print('  round%d: W1=%s W2=%s W3=%s' % (rnd + 1, row[0], row[1], row[2]))

        print('\n需求 1（同 IP 多客户端）: %s' % ('PASS' if allok else 'FAIL'))
        print('需求 2（默认 UDP）      : %s' % ('PASS' if ok2 else 'FAIL'))
        print('需求 3（env 回退地址）  : %s' % ('PASS' if ok3 else 'FAIL'))
        return 0 if (allok and ok2 and ok3 and ok4) else 1
    finally:
        for p in procs:
            try:
                p.terminate()
            except Exception:
                pass


if __name__ == '__main__':
    sys.exit(main())
