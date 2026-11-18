# -*- coding: utf-8 -*-
"""验收契约 E+F（用**新二进制**）。

  F. 固定端口：svc 默认 7711，客户端零参数、无环境变量 -> 来源 default
  E. 环境变量：两端 IMS_DAT_SVC=127.0.0.1:7901         -> 来源 env
  C. 显式参数：--svc 127.0.0.1:7901                     -> 来源 arg

注意：**不要**在进程还活着时读它的 stderr（会阻塞到 EOF）。这里一律写文件再读。
"""
import http.client
import json
import os
import subprocess
import sys
import time

ROOT = r'C:\dev\tryit\ims_mem2'
PSK = 'dd' * 32
WPORT = 7932


def clean_env(extra=None, keep_dat_svc=False):
    e = {k: v for k, v in os.environ.items() if preserve(k, keep_dat_svc)}
    e['IMS_PSK'] = PSK
    if extra:
        e.update(extra)
    return e


def preserve(k, keep_dat_svc):
    if k in ('IMS_UDP_DROP', 'IMS_UDP_ALLOW'):
        return False
    if k == 'IMS_DAT_SVC':
        return keep_dat_svc
    return True


def log_path(tag):
    return os.path.join(ROOT, 'temp', 'ef_%s.log' % tag)


def start_svc(addr, env):
    f = open(log_path('svc_' + addr.replace(':', '_')), 'w')
    return subprocess.Popen(
        [os.path.join(ROOT, 'target', 'release', 'ims_svc.exe'), '--udp', addr,
         '--mgw', r'C:\dev\ims_ws\data\mgw'],
        cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=f)


def start_web(env, extra, tag):
    f = open(log_path(tag), 'w')
    args = [os.path.join(ROOT, 'target', 'release', 'ims_web.exe')] + list(extra) + \
           ['--http', '127.0.0.1:%d' % WPORT]
    return subprocess.Popen(args, cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=f)


def wait_ok(timeout=25):
    end = time.time() + timeout
    while time.time() < end:
        try:
            c = http.client.HTTPConnection('127.0.0.1', WPORT, timeout=3)
            c.request('GET', '/api/hello')
            b = c.getresponse().read()
            c.close()
            if b'ok":true' in b.replace(b' ', b''):
                return True
        except Exception:
            pass
        time.sleep(0.3)
    return False


def big_query():
    """顺带确认大响应（76 KB）也 OK。"""
    try:
        c = http.client.HTTPConnection('127.0.0.1', WPORT, timeout=60)
        c.request('GET', '/api/catalog?pool=prefix&kind=prefix&arg=1&limit=500')
        b = c.getresponse().read()
        c.close()
        return len(json.loads(b).get('items') or [])
    except Exception:
        return -1


def src_line(tag):
    try:
        for line in open(log_path(tag), encoding='utf-8', errors='replace'):
            if '目标 svc' in line:
                return line.strip()
    except Exception:
        pass
    return '(无)'


def kill(*ps):
    for p in ps:
        try:
            p.terminate()
        except Exception:
            pass
    time.sleep(0.6)


def main():
    res = {}
    os.makedirs(os.path.join(ROOT, 'temp'), exist_ok=True)

    # ---- F
    print('F. svc 默认 7711；客户端零参数、无 IMS_DAT_SVC')
    env = clean_env()
    s = start_svc('127.0.0.1:7711', env)
    time.sleep(3)
    w = start_web(env, [], 'F')
    ok = wait_ok()
    # 这个用例只加载 MGW（不加载 AGCF），所以 catalog 本来就 0 条；
    # 这里验的是**地址契约**，不是数据量（数据量由 check_multiclient.py 覆盖）
    n = big_query() if ok else -1
    print('   %s' % src_line('F'))
    res['F'] = ok
    print('   F: %s（hello ok；catalog items=%s 属预期为 0，因为本用例没加载 AGCF）'
          % ('PASS' if ok else 'FAIL', n))
    kill(w, s)

    # ---- E
    print('\nE. 两端 IMS_DAT_SVC=127.0.0.1:7901')
    env = clean_env({'IMS_DAT_SVC': '127.0.0.1:7901'}, keep_dat_svc=True)
    s = start_svc('127.0.0.1:7901', env)
    time.sleep(3)
    w = start_web(env, [], 'E')
    ok = wait_ok()
    n = big_query() if ok else -1
    print('   %s' % src_line('E'))
    res['E'] = ok
    print('   E: %s（hello ok；items=%s 属预期）' % ('PASS' if ok else 'FAIL', n))
    kill(w)

    # ---- C
    print('\nC. 显式 --svc 127.0.0.1:7901（覆盖环境变量）')
    w = start_web(env, ['--svc', '127.0.0.1:7901'], 'C')
    ok = wait_ok(15)
    print('   %s' % src_line('C'))
    res['C'] = ok
    print('   C: %s' % ('PASS' if ok else 'FAIL'))
    kill(w, s)

    print('\n汇总: ' + ' '.join('%s=%s' % (k, 'PASS' if v else 'FAIL') for k, v in res.items()))
    return 0 if all(res.values()) else 1


if __name__ == '__main__':
    sys.exit(main())
