# -*- coding: utf-8 -*-
"""复现用户看到的 10054，并验证三件事：

  A. **旧症状**：svc 用 `--tcp`、客户端走 UDP -> 必须给出**能看懂**的提示（不再是裸 OS 错误）
  B. **修复后默认**：svc 不带参数（默认 UDP）+ 客户端不带参数 -> 正常
  C. svc `--tcp` + 客户端显式 `tcp://` -> 仍然可用（遗留路径没坏）
"""
import http.client
import json
import os
import subprocess
import sys
import time

ROOT = r'C:\dev\tryit\ims_mem2'
W = 7942
PSK = 'ee' * 32
LINK = os.path.join(ROOT, 'temp', 'agcf')
DATA = os.path.join(LINK, 'ALLME_2026*.txt')


def ensure_data():
    src = os.path.join(ROOT, '..', 'agcf_data')
    if not os.path.isdir(LINK) and os.path.isdir(src):
        subprocess.run(['cmd', '/c', 'mklink', '/J', LINK, src], capture_output=True)
    return os.path.isdir(LINK)


def env(**extra):
    e = dict(os.environ, IMS_PSK=PSK)
    e.pop('IMS_DAT_SVC', None)
    e.update(extra)
    return e


def start_svc(args, tag):
    f = open(os.path.join(ROOT, 'temp', 'e10054_svc_%s.log' % tag), 'w')
    return subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_svc.exe')] + args,
                            cwd=ROOT, env=env(), stdout=subprocess.DEVNULL, stderr=f)


def start_web(port, extra, tag):
    f = open(os.path.join(ROOT, 'temp', 'e10054_web_%s.log' % tag), 'w')
    args = [os.path.join(ROOT, 'target', 'release', 'ims_web.exe')] + list(extra) + \
           ['--http', '127.0.0.1:%d' % port]
    return subprocess.Popen(args, cwd=ROOT, env=env(), stdout=subprocess.DEVNULL, stderr=f)


def call(port, path='/api/hello'):
    try:
        c = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
        c.request('GET', path)
        b = c.getresponse().read()
        c.close()
        return json.loads(b)
    except Exception as e:
        return {'ok': False, 'error': {'message': 'HTTP 层失败: %s' % e}}


def kill(*ps):
    for p in ps:
        try:
            p.terminate()
        except Exception:
            pass
    time.sleep(0.8)


def main():
    if not ensure_data():
        print('x 缺 AGCF 夹具', file=sys.stderr)
        return 4
    res = {}
    try:
        # ---- A：旧症状（svc TCP，客户端 UDP）
        print('A. svc --tcp（监听 TCP）+ 客户端默认 UDP → 应给出可读提示')
        s = start_svc(['--tcp', '127.0.0.1:7941', '--mgw', r'C:\dev\ims_ws\data\mgw', DATA], 'tcp')
        time.sleep(15)
        w = start_web(W, ['--svc', '127.0.0.1:7941'], 'a')
        time.sleep(2)
        r = call(W)
        msg = (r.get('error') or {}).get('message', '')
        print('   返回: %s' % msg[:200])
        res['A'] = (not r.get('ok')) and ('没有 UDP 服务' in msg)
        print('   A: %s（期望含"没有 UDP 服务"）' % ('PASS' if res['A'] else 'FAIL'))
        kill(w, s)

        # ---- B：默认两边都是 UDP
        print('\nB. svc 默认（UDP）+ 客户端默认 → 应正常')
        s = start_svc(['127.0.0.1:7943', '--mgw', r'C:\dev\ims_ws\data\mgw', DATA], 'udp')
        time.sleep(15)
        w = start_web(W, ['--svc', '127.0.0.1:7943'], 'b')
        time.sleep(2)
        r = call(W)
        r2 = call(W, '/api/catalog?pool=prefix&kind=prefix&arg=1&limit=500')
        n = len(r2.get('items') or [])
        res['B'] = bool(r.get('ok')) and n == 500
        print('   hello ok=%s / 76KB items=%s' % (r.get('ok'), n))
        print('   B: %s' % ('PASS' if res['B'] else 'FAIL'))
        kill(w, s)

        # ---- C：TCP 遗留路径
        print('\nC. svc --tcp + 客户端显式 tcp:// → 应仍可用')
        s = start_svc(['--tcp', '127.0.0.1:7944', '--mgw', r'C:\dev\ims_ws\data\mgw', DATA], 'tcp2')
        time.sleep(15)
        w = start_web(W, ['--svc', 'tcp://127.0.0.1:7944'], 'c')
        time.sleep(2)
        r = call(W)
        res['C'] = bool(r.get('ok'))
        print('   hello ok=%s' % r.get('ok'))
        print('   C: %s' % ('PASS' if res['C'] else 'FAIL'))
        kill(w, s)

        print('\n汇总: ' + ' '.join('%s=%s' % (k, 'PASS' if v else 'FAIL') for k, v in res.items()))
        return 0 if all(res.values()) else 1
    finally:
        pass


if __name__ == '__main__':
    sys.exit(main())
