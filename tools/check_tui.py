# -*- coding: utf-8 -*-
"""服务/客户端回归：ims_svc --selftest（进程内）与 ims_tui --tcp --selftest（经传输）必须一致。

  2026-11-15 拆分程序入口后：服务端=ims_svc（引擎+dispatch+accept），客户端=ims_tui（UI+解析+TCP）。
  原 embedded 形态取消，对拍两方改为：
    A  ims_svc --selftest <files>   —— 不走传输，直接 dispatch
    B  ims_svc <addr> <files> 常驻（默认 UDP）；ims_tui --tcp udp://<addr> --selftest —— 经 UDP+AEAD

用法：
    python tools/check_tui.py            # 自动完成 A、B 并比对，收尾

断言：
  A 两种形态的目录/对象/链接结果逐行相同（去掉耗时与"形态/导入"行）
  B tcp 形态命中数非空（服务真的在答）
  C 服务端日志出现"客户端接入/断开"（连接生命周期正确）
退出码 0 = 通过。
"""
import os
import re
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE_SVC = os.path.join(ROOT, 'target', 'release', 'ims_svc.exe')
EXE_TUI = os.path.join(ROOT, 'target', 'release', 'ims_tui.exe')
DATA = r'C:\dev\tryit\agcf_data'
ADDR = '127.0.0.1:7713'


def files():
    return [os.path.join(DATA, x) for x in sorted(os.listdir(DATA))
            if x.startswith('ALLME_2026') and x.endswith('.txt')]


def strip(out):
    """去掉非确定性行：形态/导入耗时/耗时后缀/hello 原样保留"""
    keep = []
    for line in out.splitlines():
        if line.startswith('形态:') or line.startswith('导入完成'):
            continue
        line = re.sub(r' · [0-9.]+ ms（含传输）', '', line)
        line = re.sub(r' · [0-9.]+ ms', '', line)
        keep.append(line.strip())
    return keep


def main():
    fs = files()
    if not fs:
        print('缺数据文件', file=sys.stderr)
        return 2
    for exe in (EXE_SVC, EXE_TUI):
        if not os.path.exists(exe):
            print(f'缺 {exe}（先 cargo build --release -p ims_svc -p ims_tui）', file=sys.stderr)
            return 2

    print('== A: ims_svc --selftest（进程内 dispatch）==')
    a = subprocess.run([EXE_SVC, '--selftest', *fs], capture_output=True, text=True)
    if a.returncode != 0:
        print('进程内自检失败:', a.stderr[-300:])
        return 1
    inline = strip(a.stdout)
    for l in inline:
        print('  ' + l)

    print('\n== B: ims_svc 常驻 + ims_tui --tcp --selftest ==')
    errlog = os.path.join(ROOT, 'temp', 'svc_check.log')
    os.makedirs(os.path.dirname(errlog), exist_ok=True)
    with open(errlog, 'w', encoding='utf-8') as ef:
        srv = subprocess.Popen([EXE_SVC, ADDR, *fs], stderr=ef, stdout=subprocess.DEVNULL)
        try:
            # 就绪：不猜协议，直接重试客户端自检（svc 默认 UDP，TCP 探测探不到）
            b = None
            for _ in range(20):
                time.sleep(3)
                b = subprocess.run([EXE_TUI, '--tcp', 'udp://' + ADDR, '--selftest'],
                                   capture_output=True, text=True)
                # 就绪 = 自检跑通**且**真的查到数据（只有 returncode 会被'服务还没起来'骗过）
                if b.returncode == 0 and '匹配' in b.stdout:
                    break
            if b is None or b.returncode != 0:
                print('客户端自检失败:', (b.stderr[-300:] if b else '(未运行)'))
                return 1
            tcp = strip(b.stdout)
            for l in tcp:
                print('  ' + l)
        finally:
            srv.terminate()
            try:
                srv.wait(timeout=5)
            except subprocess.TimeoutExpired:
                srv.kill()

    fails = 0
    if inline != tcp:
        print('\nx 两种形态结果不一致：')
        for i, (x, y) in enumerate(zip(inline, tcp)):
            if x != y:
                print(f'  行{i}:\n    inline: {x}\n    tcp   : {y}')
        if len(inline) != len(tcp):
            print(f'  行数不同 {len(inline)} vs {len(tcp)}')
        fails += 1
    if not any('匹配' in l for l in tcp):
        print('x tcp 形态没有任何匹配行')
        fails += 1
    if not any('匹配' in l for l in inline):
        print('x 进程内自检没有任何匹配行')
        fails += 1
    log = open(errlog, encoding='utf-8').read() if os.path.exists(errlog) else ''
    # UDP 没有"连接断开"事件（无连接协议），所以只在 TCP 模式下才要求"断开"
    is_udp = 'UDP' in log
    need = ['客户端接入'] if is_udp else ['客户端接入', '客户端断开']
    missing = [x for x in need if x not in log]
    if missing:
        print('x 服务端日志缺少 %s（生命周期不完整）' % '/'.join(missing))
        fails += 1

    if fails:
        print(f'\nx check_tui 失败 {fails} 项')
        return 1
    print(f'\nok 进程内与经网络的结果一致（{len(inline)} 行）· 服务连接生命周期正常')
    return 0


if __name__ == '__main__':
    sys.exit(main())
