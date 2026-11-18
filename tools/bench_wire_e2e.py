# -*- coding: utf-8 -*-
"""端到端：同一批查询走 TCP 与 UDP(+AEAD)，比较每请求耗时。

HTTP 前端（ims_web）两边相同，所以**差值**就是「加密 + UDP 停等」的代价。
用 keep-alive 连接把 HTTP 握手摊掉。
"""
import http.client
import os
import statistics
import subprocess
import sys
import time

ROOT = r'C:\dev\tryit\ims_mem2'
PSK = 'cd' * 32
N = 300


def start(procs, svc_args, web_args, port):
    env = dict(os.environ, IMS_PSK=PSK, IMS_DAT_SVC='127.0.0.1:%d' % (port - 1))
    s = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_svc.exe')] + svc_args,
                         cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(2.5)
    w = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_web.exe')] +
                         web_args + ['--http', '127.0.0.1:%d' % port],
                         cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(2.0)
    procs += [w, s]
    return port


def bench(port, path, n=N):
    c = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
    # 预热
    for _ in range(20):
        c.request('GET', path)
        c.getresponse().read()
    ts = []
    for _ in range(n):
        t0 = time.perf_counter()
        c.request('GET', path)
        r = c.getresponse()
        body = r.read()
        ts.append((time.perf_counter() - t0) * 1e6)
    c.close()
    ts.sort()
    return statistics.median(ts), ts[int(len(ts) * 0.95)], len(body)


def main():
    procs = []
    try:
        tcp = start(procs, ['--mgw', r'C:\dev\ims_ws\data\mgw'],
                    ['--svc', '127.0.0.1:7851'], 7852)
        udp = start(procs, ['--udp', '--mgw', r'C:\dev\ims_ws\data\mgw'],
                    ['--svc', 'udp://127.0.0.1:7853'], 7854)
        cases = [
            ('/api/hello', '极小响应'),
            ('/api/object?io=Mgw&ne=-1&key=lh1', 'Mgw 对象'),
            ('/api/catalog?pool=prefix&kind=prefix&arg=7&limit=50', 'catalog 50 条'),
        ]
        print('%-34s %10s %10s %8s | %8s %8s %8s' %
              ('请求', 'TCP p50', 'UDP p50', 'p50 差', 'TCP p95', 'UDP p95', '字节'))
        print('-' * 100)
        for path, label in cases:
            t50, t95, n = bench(tcp, path)
            u50, u95, _ = bench(udp, path)
            print('%-30s %9.1fµs %9.1fµs %+7.1fµs | %7.1fµs %7.1fµs %8d' %
                  (label, t50, u50, u50 - t50, t95, u95, n))
    finally:
        for p in procs:
            try:
                p.terminate()
            except Exception:
                pass
    return 0


if __name__ == '__main__':
    sys.exit(main())
