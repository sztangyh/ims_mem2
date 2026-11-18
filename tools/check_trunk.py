import sqlite3, sys
def _cur_db():
    import os
    p = r'temp\cur_db.txt'
    try:
        with open(p, encoding='utf-8') as f:
            v = f.read().strip()
        if v:
            return v
    except OSError:
        pass
    return r'temp\proj.db'


db = sqlite3.connect(_cur_db())
rows = db.execute('select count(*) from trunk_cur').fetchone()[0]
by_kind = db.execute('select kind, count(*), count(DN) from trunk_cur group by 1 order by 1').fetchall()
dn = db.execute('select count(DN) from trunk_cur').fetchone()[0]
hit = db.execute("select count(*) from trunk_cur where DN='K''28811399'").fetchone()[0]
pre = db.execute("select count(*) from trunk_cur where DN like 'K''2881%'").fetchone()[0]
print(f'trunk_cur {rows} 行 / DN 非空 {dn} / 反查 K\'28811399 → {hit} 条 / 号段 K\'2881 → {pre} 条')
print('  按族:', by_kind)
ok = rows == 4916 and dn == 4405 and hit >= 1 and pre == 31
sys.exit(0 if ok else 1)
