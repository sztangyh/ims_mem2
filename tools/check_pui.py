import sqlite3, re, sys
import os

def _cur_db():
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    p = os.path.join(here, 'temp', 'cur_db.txt')
    try:
        with open(p, encoding='utf-8') as f:
            v = f.read().strip()
        if v:
            return v if os.path.isabs(v) else os.path.join(here, v)
    except OSError:
        pass
    return os.path.join(here, 'temp', 'proj.db')


db = sqlite3.connect(_cur_db())
row = db.execute("select PUI from cur_ASBR where ne='AGCF63' limit 1").fetchone()
v = row[0] if row else ''
n = db.execute("select count(*) from cur_ASBR where PUI like 'sip:%@%'").fetchone()[0]
tot = db.execute("select count(*) from cur_ASBR").fetchone()[0]
print(f'样例 {v} / 形如 sip:...@... 的行 {n}/{tot}（其余为原样保存的异常形状）')
ok = bool(re.match(r'^sip:\+?[0-9]+@.+', v or '')) and n >= tot - 10
sys.exit(0 if ok else 1)
