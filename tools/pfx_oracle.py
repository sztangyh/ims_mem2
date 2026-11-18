# -*- coding: utf-8 -*-
r"""CNACLD 最长匹配的独立 oracle（P7）：从原文取 PFX，暴力求最长匹配。

口径与 `examples/pfx_lpm_dump.rs` 完全一致：
- 对每个字冠 P，派生 3 个拨号串：P、P+"9"、P 末位改 8（9 则改 0）；
- 对每个拨号串求"配置里最长的、且是它前缀的"字冠；没有则输出 `-`；
- 输出 `ne|dial|命中字冠`。

用法：python tools/pfx_oracle.py ..\agcf_data\ALLME_*.txt
"""
import glob
import re
import sys

USE_ME = re.compile(rb'^USE ME:MEID=(\d+)')
ADD_ME = re.compile(rb'^ADD ME:(.*);\s*$')
ADD_CNACLD = re.compile(rb'^(?:ADD|MOD|SET) CNACLD:(.*);\s*$')


def split_params(body):
    out, i, start, inq = [], 0, 0, False
    while i < len(body):
        c = body[i]
        if c == 0x22:
            inq = not inq
        elif c == 0x2C and not inq:
            out.append(body[start:i])
            start = i + 1
        i += 1
    out.append(body[start:])
    kv = {}
    for seg in out:
        if b'=' not in seg:
            continue
        k, v = seg.split(b'=', 1)
        kv[k.strip().upper()] = v.strip().strip(b'"')
    return kv


def main(paths):
    files = []
    for p in paths:
        files.extend(sorted(glob.glob(p)) if any(ch in p for ch in '*?[') else [p])
    per_ne = {}
    for path in files:
        data = open(path, 'rb').read()
        names = {}
        for line in data.split(b'\n'):
            m = ADD_ME.match(line.rstrip(b'\r \t'))
            if not m:
                continue
            kv = split_params(m.group(1))
            if b'MEID' in kv and b'MENAME' in kv and kv[b'MEID'].isdigit():
                names[int(kv[b'MEID'])] = kv[b'MENAME'].decode('gbk', 'replace')
        meid = None
        for line in data.split(b'\n'):
            line = line.rstrip(b'\r \t')
            m = USE_ME.match(line)
            if m:
                meid = int(m.group(1))
                continue
            m = ADD_CNACLD.match(line)
            if not m or meid is None:
                continue
            ne = names.get(meid)
            if not ne:
                continue
            kv = split_params(m.group(1))
            pfx = kv.get(b'PFX')
            if not pfx:
                continue
            s = pfx.decode('latin-1')
            if s.startswith("K'"):
                s = s[2:]
            if not s:
                continue
            per_ne.setdefault(ne, set()).add(s)

    for ne in sorted(per_ne):
        pfx = per_ne[ne]
        dials = set()
        for p in pfx:
            dials.add(p)
            dials.add(p + '9')
            last = p[-1]
            dials.add(p[:-1] + ('0' if last == '9' else '8'))
        for d in sorted(dials):
            best = '-'
            for p in pfx:
                if d.startswith(p) and (best == '-' or len(p) > len(best) - 2):
                    if best == '-' or len(p) > len(best) - 2:
                        best = "K'" + p
            print(f'{ne}|{d}|{best}')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
