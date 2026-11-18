# -*- coding: utf-8 -*-
"""验证 NACK 补片：在**客户端收方向**按比例丢包，76 KB 的 catalog 仍须完整返回。

对照组：
  A. 不丢（基线）           → 完整、0 次 NACK
  B. 10% 丢包              → 完整、有 NACK
  C. 30% 丢包              → 完整、NACK 更多
  D. 60% 丢包（压力）       → 仍应尽量完整；若失败要报出来（这是预期边界，不是 bug）
"""
import http.client
import json
import os
import subprocess
import sys
import time

ROOT = r'C:\dev\tryit\ims_mem2'
PORT_SVC = 7881
PORT_WEB = 7882
PATH = '/api/catalog?pool=prefix&kind=prefix&arg=1&limit=500'
EXPECT_ITEMS = 500


def start_svc():
    env = dict(os.environ, IMS_PSK='ef' * 32, IMS_DAT_SVC='127.0.0.1:%d' % PORT_SVC)
    s = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_svc.exe'), '--udp',
                          '--mgw', r'C:\dev\ims_ws\data\mgw',
                          os.path.join(ROOT, 'temp', 'agcf3', 'ALLME_2026*.txt')],
                         cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(14)
    return s


def start_web(drop):
    env = dict(os.environ, IMS_PSK='ef' * 32, IMS_DAT_SVC='127.0.0.1:%d' % PORT_SVC)
    if drop:
        env['IMS_UDP_DROP'] = str(drop)
    w = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_web.exe'),
                          '--svc', 'udp://127.0.0.1:%d' % PORT_SVC,
                          '--http', '127.0.0.1:%d' % PORT_WEB],
                         cwd=ROOT, env=env, stdout=subprocess.DEVNULL,
                         stderr=subprocess.PIPE)
    time.sleep(2)
    return w


def run_case(label, drop, runs=3):
    print('\n=== %s（IMS_UDP_DROP=%s）===' % (label, drop or '未设'))
    ok_all = True
    for i in range(runs):
        w = start_web(drop)
        try:
            c = http.client.HTTPConnection('127.0.0.1', PORT_WEB, timeout=120)
            t0 = time.perf_counter()
            c.request('GET', PATH)
            r = c.getresponse()
            body = r.read()
            ms = (time.perf_counter() - t0) * 1000
            c.close()
            try:
                items = len(json.loads(body).get('items') or [])
            except Exception:
                items = -1
            good = items == EXPECT_ITEMS
            ok_all &= good
            print('  run%d: %6d B / items=%-4s %s  %7.1f ms' %
                  (i + 1, len(body), items, 'ok' if good else 'x 不完整', ms))
            # 把 [drop-test] 那行打出来
            time.sleep(0.3)
        finally:
            w.terminate()
            try:
                err = w.stderr.read().decode('utf-8', 'replace')
            except Exception:
                err = ''
            for line in err.splitlines():
                if '[drop-test]' in line:
                    print('         ' + line.strip())
        time.sleep(0.8)
    return ok_all


def main():
    os.makedirs(os.path.join(ROOT, 'temp'), exist_ok=True)
    link = os.path.join(ROOT, 'temp', 'agcf3')
    if not os.path.exists(link):
        subprocess.run(['cmd', '/c', 'mklink', '/J', link, os.path.join(ROOT, '..', 'agcf_data')],
                       capture_output=True)
    svc = start_svc()
    try:
        a = run_case('A 基线（不丢包）', None)
        b = run_case('B 10% 丢包', 10)
        c = run_case('C 30% 丢包', 30)
        d = run_case('D 60% 丢包（压力，允许失败）', 60, runs=2)
        print('\n结果：A=%s B=%s C=%s D=%s' %
              ('PASS' if a else 'FAIL', 'PASS' if b else 'FAIL',
               'PASS' if c else 'FAIL', 'PASS' if d else 'PARTIAL'))
        return 0 if (a and b and c) else 1
    finally:
        svc.terminate()


if __name__ == '__main__':
    sys.exit(main())
