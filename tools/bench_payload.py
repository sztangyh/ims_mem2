# -*- coding: utf-8 -*-
"""量一下真实响应有多大 —— 判断 65,519 B 的 UDP 单包上限是不是已经在踩线。"""
import http.client
import json
import os
import subprocess
import time

ROOT = r'C:\dev\tryit\ims_mem2'
env = dict(os.environ, IMS_PSK='cd' * 32, IMS_DAT_SVC='127.0.0.1:7861')

s = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_svc.exe'),
                      '--mgw', r'C:\dev\ims_ws\data\mgw', r'..\agcf_data\ALLME_2026*.txt'],
                     cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(14)
w = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_web.exe'),
                      '--svc', '127.0.0.1:7861', '--http', '127.0.0.1:7862'],
                     cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(2)

cases = [
    ('catalog prefix "7" limit=500', '/api/catalog?pool=prefix&kind=prefix&arg=7&limit=500'),
    ('catalog prefix "1" limit=500', '/api/catalog?pool=prefix&kind=prefix&arg=1&limit=500'),
    ('catalog subscriber "1" limit=500', '/api/catalog?pool=subscriber&kind=prefix&arg=1&limit=500'),
    ('catalog trunk "1" limit=500', '/api/catalog?pool=trunk&kind=prefix&arg=1&limit=500'),
    ('object PrefixProfile 7552', '/api/object?io=PrefixProfile&ne=1&key=7552'),
    ('object Subscriber', '/api/object?io=Subscriber&ne=1&key=7552120001'),
]
LIMIT = 65519
print('%-36s %10s %10s %s' % ('请求', '字节', '超限?', '条数'))
print('-' * 76)
try:
    c = http.client.HTTPConnection('127.0.0.1', 7862, timeout=40)
    for label, path in cases:
        c.request('GET', path)
        r = c.getresponse()
        body = r.read()
        try:
            n = len(json.loads(body).get('items') or [])
        except Exception:
            n = '-'
        flag = 'x 超限' if len(body) > LIMIT else 'ok'
        print('%-32s %9d B %10s %8s' % (label, len(body), flag, n))
finally:
    for p in (w, s):
        try:
            p.terminate()
        except Exception:
            pass
