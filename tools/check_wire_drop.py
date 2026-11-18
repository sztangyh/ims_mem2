# -*- coding: utf-8 -*-
"""验证「非白名单 / 加密错」的包被**静默丢弃**：发垃圾过去，必须一个字节都不回。"""
import os
import socket
import subprocess
import sys
import time

ROOT = r'C:\dev\tryit\ims_mem2'
PORT = 7831
PSK = 'ab' * 32
env = dict(os.environ, IMS_PSK=PSK, IMS_DAT_SVC='127.0.0.1:%d' % PORT)

p = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_svc.exe'), '--udp',
                      '--mgw', r'C:\dev\ims_ws\data\mgw'],
                     cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(3.0)

cases = [
    ('随机 30 字节', bytes(range(30))),
    ('魔数对但版本错', b'IMSU' + bytes([9]) + b'\x01' + b'\x00' * 40),
    ('魔数对但密文垃圾（40 字节）', b'IMSU' + bytes([1, 1]) + b'\x07\x00\x00\x00' + b'\x00' * 8
     + b'\x11' * 24 + b'\x10\x00' + b'\x22' * 16),
    ('全零 100 字节', b'\x00' * 100),
]

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.settimeout(1.0)
ok = True
for name, payload in cases:
    s.sendto(payload, ('127.0.0.1', PORT))
    try:
        data, _ = s.recvfrom(65535)
        print('x %-30s 居然回了 %d 字节 —— 破坏了"静默丢弃"' % (name, len(data)))
        ok = False
    except socket.timeout:
        print('   ok %-30s 无响应（1s 超时）' % name)

# 反证：正确加密必须能通（这里不实现 XChaCha20，改用 web 走一次真请求）
p2 = subprocess.Popen([os.path.join(ROOT, 'target', 'release', 'ims_web.exe'),
                       '--svc', 'udp://127.0.0.1:%d' % PORT, '--http', '127.0.0.1:7832'],
                      cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(2.0)
import urllib.request
try:
    with urllib.request.urlopen('http://127.0.0.1:7832/api/hello', timeout=20) as r:
        body = r.read().decode('utf-8', 'replace')
    print('   ok 反证：正确加密的请求有响应（%d 字节）' % len(body))
except Exception as e:
    print('x 反证失败：%s' % e)
    ok = False

for proc in (p2, p):
    try:
        proc.terminate()
    except Exception:
        pass
sys.exit(0 if ok else 1)
