# -*- coding: utf-8 -*-
"""中文词典（中文名 / 取值解释）：从冻结库导出 `zh.tsv`。

来源：`C:/dev/ims_ws/data/ims_asset/library.db` 的 5 张 `zh_*` 表
（`slice9_scd2.py::build_zh` 的产物，行数 503 / 5,416 / 2,661 / 4,206 / 3,393）。

为什么导成 TSV 而不是让 core 读 sqlite：
  * core 现在只有 2 个依赖（`ims_mem2_macros` + `encoding_rs`），读 sqlite 要引 `rusqlite`
    —— 那是 `crates/proj` 的事，core 不该为一张显示用的字典破这条纪律；
  * 与 `defaults.tsv`（A4）同一形态：一个可 diff、可 `--check` 复现的文本产物。

格式（6 列，TAB 分隔；转义 `\\\\` → `\\`、`\\t` → TAB、`\\n` → LF）：

    kind  k1     k2      k3      text        extra
    M     MOC    OP      -       cmd_cn      name_cn      ← zh_moc（命令的中文名）
    P     MOC    PARAM   -       param_cn    -            ← zh_param（一级：按 MOC）
    Q     PARAM  -       -       param_cn    n_moc:amb    ← zh_param_any（二级：跨 MOC 兜底）
    V     MOC    PARAM   VALUE   value_cn    -            ← zh_value（枚举取值的中文解释）
    W     PARAM  VALUE   -       value_cn    n_moc:amb    ← zh_value_any（二级兜底）
    S     MOC    PARAM   SWITCH  switch_cn   src          ← 开关名（位名）的中文说明，见下

`kind=Q` / `kind=W` 的 `extra` 是 `n_moc:ambiguous`（`slice9_scd2.py` 原样带过来的两列），
用来判断"这个中文名在多少张 MOC 下出现过、是否撞名" —— 展示时可以选择只信 `ambiguous=0` 的。

## `kind=S`：开关型参数（`Bits`）的位名中文 —— 三个来源的裁决

开关名是**通用短词**（`SVR2`/`AEC`/`CCR1`/`TIN`），同一个词在不同 `(MOC, PARAM)` 下含义不同。
实测 ims_mem2 的 1,058 个开关名里，**521 个（49%）是"跨多个位序空间"的多义词**。三个候选来源：

| 来源 | 定位方式 | 命中/1,058 | 含汉字 | 结论 |
|---|---|---|---|---|
| `mml_doc/agcf_doc/manual_switch_meanings.json` | `(moc,param,switch)` 三元组，**人工裁决**（15 个参数 / 232 位，每条带 `note`） | 232 | 232 | **最准**，优先 |
| `agcf_mml.db::switch_meanings.doc_desc` | `(moc,param,switch)` 三元组，文档自动解析 | 469 | 469 | **同义率 100%**（与 manual 逐条比 332/332 同义），第二优先 |
| `ims_asset/switch_vocab.json::means_cn` | **只按位词**（跨参数聚合），`n_spaces` = 出现在几个位序空间 | 595 | 595 | **只在 `n_spaces==1` 时可信**（此时与 manual 一致）；多义位词与 manual 仅 16% 同义 |
| `switch_meanings.doc_option` | 三元组，文档里的"选项名" | 469 | **129** | **不用**：469 里 340 行是英文（`No EC cancellation`），且那 129 行汉字与 `doc_desc` 重复（增量 +0） |

所以本生成器只收前三者的**严格交集链**：`manual` → `doc_desc` → `vocab`（**仅 `n_spaces==1`**）。
`extra` 记来源（`manual` / `doc` / `vocab`），便于审计与将来换源。

实测落地：**516 / 1,058 = 48.8%**；剩 527 个开关名分布在 15 个参数上，**文档里没有逐位解释**
（`N1TGDEF.IRCM` 128、`CVSTATYPE.INTRA` 113 / `Q850` 62 / `SIP` 43 …都是"号码/原因值位"）。
另有 **97 个多义位词被显式放弃**（不是漏掉）：按词套用会把 `APRATGDEF.SVRCTRL.SVR2`（出中继Setup…
强制置 speech）说成 `N1TGDEF` 的"接续成功时由中继送回铃音"——同一词五个参数五种含义。

用法（S 层的"冻结 / 同步"两种模式）：
    python tools/gen_zh.py --check            # 离线：重算 zh.tsv 并与磁盘比（读仓内快照）
    python tools/gen_zh.py --write            # 离线：重算并写 zh.tsv
    python tools/gen_zh.py --diff             # **有外部输入时**：外部源重裁决 vs 仓内快照的差异（只报不写）
    python tools/gen_zh.py --refresh --write  # **有外部输入时**：重裁决 → 重写 switch_meanings.tsv → 重写 zh.tsv

外部源更新（尤其 `manual_switch_meanings.json`）后的同步流程见
`spec/SWITCH-NAMES-AUDIT.md` §9：`--diff` 看差异 → `--refresh --write` → 跑全量门。
⚠ 删掉 manual 里的一条**不会消失**，而是降级到 `doc_desc`/`vocab`（`--diff` 会报成"修改"）。

退出码：0 成功或无差异 / 3 `--check` 不一致或 `--diff` 有差异 / 4 源库、外部源或快照缺失。
"""
import argparse
import json
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'zh.tsv')
DEFAULT_DB = os.path.join('C:/dev/ims_ws', 'data', 'ims_asset', 'library.db')
DEFAULT_DOCDB = os.path.join('C:/dev/tryit', 'mml_doc', 'agcf_doc', 'agcf_mml.db')
DEFAULT_MANUAL = os.path.join('C:/dev/tryit', 'mml_doc', 'agcf_doc', 'manual_switch_meanings.json')
DEFAULT_VOCAB = os.path.join('C:/dev/ims_ws', 'data', 'ims_asset', 'switch_vocab.json')
DEFAULT_SWDICT = os.path.join('C:/dev/ims_ws', 'data', 'ims_asset', 'switch_dict.json')
#: S 层的**冻结快照**（仓内）。默认只读它 → `--check` 完全离线；
#: 只有 `--refresh` 才去读上面那四个外部输入并重写它。
DEFAULT_SWITCH_TSV = os.path.join(ROOT, 'switch_meanings.tsv')


def has_cn(x):
    return bool(x) and bool(re.search(r'[\u4e00-\u9fff]', str(x)))


def esc(s):
    if s is None:
        return ''
    return (str(s).replace('\\', '\\\\')
            .replace('\t', '\\t')
            .replace('\n', '\\n')
            .replace('\r', ''))


def unesc(s):
    out, i = [], 0
    while i < len(s):
        if s[i] == '\\' and i + 1 < len(s):
            i += 1
            out.append({'t': '\t', 'n': '\n', '\\': '\\'}.get(s[i], s[i]))
        else:
            out.append(s[i])
        i += 1
    return ''.join(out)


def rows_of(con, sql):
    try:
        return con.execute(sql).fetchall()
    except sqlite3.Error as e:
        print('x 读不到表：%s（%s）' % (sql, e), file=sys.stderr)
        return None


def cmd_diff(docdb, manual, vocab, swdict, switch_tsv):
    """`--diff`：外部输入重新裁决的结果 vs 仓内快照，**只报不写**。

    外部源（尤其 `manual_switch_meanings.json`）更新后先跑这个，看清"会变什么"再决定是否
    `--refresh`。退出码：0 = 无差异；3 = 有差异（与 `--check` 的 3 语义一致）。
    """
    snap, old_stats = load_switch_snapshot(switch_tsv)
    if snap is None:
        return 4
    src, new_stats = build_switch(docdb, manual, vocab, swdict, ims_mem2_bits())
    if src is None:
        return 4

    added = sorted(k for k in src if k not in snap)
    removed = sorted(k for k in snap if k not in src)
    changed = sorted(k for k in src if k in snap and src[k] != snap[k])

    print('快照 %s：%d 行' % (os.path.basename(switch_tsv), len(snap)))
    print('重新裁决  ：%d 行' % len(src))
    print('来源分布  ：%s  →  %s' % (_dist(old_stats), _dist(new_stats)))
    want = ims_mem2_bits()
    cov_old = sum(1 for k in want if k in snap)
    cov_new = sum(1 for k in want if k in src)
    print('本仓 1,058 个开关名的覆盖：%d → %d（%+.0f）'
          % (cov_old, cov_new, cov_new - cov_old))

    if not (added or removed or changed):
        print('\nok 无差异：外部源没有带来任何变化（无需 --refresh）')
        return 0

    print('\n差异：新增 %d / 修改 %d / 消失 %d' % (len(added), len(changed), len(removed)))
    for tag, keys in (('新增', added), ('修改', changed), ('消失', removed)):
        if not keys:
            continue
        print('\n  [%s] %d 条，前 8：' % (tag, len(keys)))
        for k in keys[:8]:
            o = snap.get(k)
            n = src.get(k)
            if tag == '新增':
                print('    %s/%s/%s  = %s（%s）' % (k[0], k[1], k[2], n[0][:40], n[1]))
            elif tag == '消失':
                print('    %s/%s/%s  （原 %s / %s）' % (k[0], k[1], k[2], o[0][:40], o[1]))
            else:
                print('    %s/%s/%s\n      旧: %s（%s）\n      新: %s（%s）'
                      % (k[0], k[1], k[2], o[0][:50], o[1], n[0][:50], n[1]))
    print('\n提示：确认无误后跑 `python tools/gen_zh.py --refresh --write`，'
          '再跑全量门（它会重导出投影 —— `zh_switch` 表要先更新，否则 check_zh 的'
          '"6 张 zh_* 表行数相等" 会红）。')
    return 3


def _dist(st):
    return 'manual %d / doc %d / vocab %d' % (st.get('switch_manual', 0),
                                              st.get('switch_doc', 0),
                                              st.get('switch_vocab', 0))


def build(db, docdb, manual, vocab, swdict, switch_tsv, refresh):
    """组装 `zh.tsv` 的正文（7 层：M/P/Q/V/W 来自 `library.db`，S 来自冻结快照）。

    S 层两条路（[`load_switch_snapshot`] / [`build_switch`]）：
      * 默认：读仓内冻结快照 `switch_meanings.tsv` —— **不需要** docdb / manual / vocab / swdict；
      * `--refresh`：读那 4 个外部输入重新裁决，并**重写**快照。

    ⚠ 剩下那一个外部依赖是 **`library.db`**（M/P/Q/V/W 五层的源），与 S 层无关；
    S 层"冻结"的**有效证明在 `tools/check_zh.py`**（只读 `switch_meanings.tsv` 与 `zh.tsv`
    两个仓内文件，离线可跑，且在验收门里）。
    """
    if not os.path.exists(db):
        print('x 源库不存在：%s' % db, file=sys.stderr)
        print('  注意：它只影响 M/P/Q/V/W 五层；S 层（开关名）已冻结在仓内'
              ' switch_meanings.tsv，不依赖它。', file=sys.stderr)
        return None
    con = sqlite3.connect('file:%s?mode=ro' % db.replace('\\', '/'), uri=True)
    lines = []
    stats = {}

    r = rows_of(con, 'SELECT moc, op, cmd_cn, name_cn FROM zh_moc')
    if r is None:
        return None
    for moc, op, cmd_cn, name_cn in r:
        lines.append('M\t%s\t%s\t\t%s\t%s' % (esc(str(moc).upper()), esc(str(op).upper()),
                                              esc(cmd_cn), esc(name_cn)))
    stats['zh_moc'] = len(r)

    r = rows_of(con, 'SELECT moc, param, param_cn FROM zh_param')
    if r is None:
        return None
    for moc, par, cn in r:
        lines.append('P\t%s\t%s\t\t%s\t' % (esc(str(moc).upper()), esc(str(par).upper()), esc(cn)))
    stats['zh_param'] = len(r)

    r = rows_of(con, 'SELECT param, param_cn, n_moc, ambiguous FROM zh_param_any')
    if r is None:
        return None
    for par, cn, n_moc, amb in r:
        lines.append('Q\t%s\t\t\t%s\t%d:%d' % (esc(str(par).upper()), esc(cn),
                                               int(n_moc or 0), int(amb or 0)))
    stats['zh_param_any'] = len(r)

    r = rows_of(con, 'SELECT moc, param, value, value_cn FROM zh_value')
    if r is None:
        return None
    for moc, par, val, cn in r:
        lines.append('V\t%s\t%s\t%s\t%s\t' % (esc(str(moc).upper()), esc(str(par).upper()),
                                              esc(val), esc(cn)))
    stats['zh_value'] = len(r)

    r = rows_of(con, 'SELECT param, value, value_cn, n_moc, ambiguous FROM zh_value_any')
    if r is None:
        return None
    for par, val, cn, n_moc, amb in r:
        lines.append('W\t%s\t%s\t\t%s\t%d:%d' % (esc(str(par).upper()), esc(val), esc(cn),
                                                 int(n_moc or 0), int(amb or 0)))
    stats['zh_value_any'] = len(r)
    con.close()

    # ---- kind=S：开关名（位名）中文 ----
    # 默认读仓内冻结快照（离线）；`--refresh` 才去读外部输入重新裁决并写回快照。
    if refresh:
        sw, sw_stats = build_switch(docdb, manual, vocab, swdict, ims_mem2_bits())
        if sw is None:
            return None
        if write_switch_snapshot(switch_tsv, sw, sw_stats) is None:
            return None
    else:
        sw, sw_stats = load_switch_snapshot(switch_tsv)
        if sw is None:
            return None
    for (moc, par, name), (cn, src) in sw.items():
        lines.append('S\t%s\t%s\t%s\t%s\t%s' % (esc(moc), esc(par), esc(name), esc(cn), src))
    stats.update(sw_stats)

    # 行序无关的确定性排序（kind 序固定 = 文件书写序）
    order = {'M': 0, 'P': 1, 'Q': 2, 'V': 3, 'W': 4, 'S': 5}
    lines.sort(key=lambda s: (order[s[0]], s))
    head = ('# 中文词典（tools/gen_zh.py 生成，勿手改）\n'
            '# kind\tk1\tk2\tk3\ttext\textra   —— 见 tools/gen_zh.py 头部说明\n')
    return head + '\n'.join(lines) + '\n', stats


def ims_mem2_bits():
    """本仓 44 个 `Bits` 参数的 `(MOC, PARAM, 开关名)` 全集（1,058 条）。

    解析 `crates/core/src/schema_gen_data.rs` —— 与本仓 `gen_defaults.py` / `infer_keys.py`
    同一做法（那两个工具也直接读这个文件）。有了它，`switch_vocab` 才能给"本仓缺、
    但位词无歧义"的位置兜底。
    """
    src = os.path.join(ROOT, 'crates', 'core', 'src', 'schema_gen_data.rs')
    if not os.path.exists(src):
        return set()
    s = open(src, encoding='utf-8', errors='replace').read()
    m = re.search(r'pub static BITS_TABLES: &\[&\[&str\]\] = &\[(.*?)\n\];', s, re.S)
    if not m:
        return set()
    tables, cc = [], None
    for line in m.group(1).splitlines():
        line = line.strip()
        cm = re.match(r'//\s*([A-Z0-9_]+)\.([A-Z0-9_]+)', line)
        if cm:
            cc = (cm.group(1), cm.group(2))
            continue
        am = re.match(r'&\[(.*)\],?$', line)
        if am:
            tables.append((cc, re.findall(r'"([^"]*)"', am.group(1))))
    out = set()
    for mm in re.finditer(r'mml_table!\s*\{\s*\n\s*([A-Z0-9_]+)\s*,\s*\w+\s*,\s*\n(.*?)\n\}\n',
                          s, re.S):
        moc = mm.group(1)
        for fm in re.finditer(r'^\s{4}(?:r#)?(\w+)\s*:\s*Bits\((\d+)\)', mm.group(2), re.M):
            par = fm.group(1).upper()
            for nm in tables[int(fm.group(2))][1]:
                out.add((moc, par, nm.upper()))
    return out


def switch_dict_triples(path):
    """`switch_dict.json` 的 `(MOC, PARAM, 位名)`（33 个位序空间 / 680 位）。"""
    out = set()
    if not os.path.exists(path):
        return out
    for e in json.load(open(path, encoding='utf-8')):
        par = str(e.get('param', '')).upper()
        for moc in (e.get('mocs') or [e.get('scope', '')]):
            for nm in (e.get('bits') or []):
                out.add((str(moc).upper(), par, str(nm).upper()))
    return out


def load_switch_snapshot(path):
    """读仓内冻结快照 `switch_meanings.tsv` → `({(moc,param,switch): (中文, src)}, stats)`。

    行格式：`moc \t param \t switch_name \t 中文 \t src`（`#` 开头是注释）。
    这是 `--check` 的**唯一**依赖 —— 不碰任何外部库/外部 json。
    """
    if not os.path.exists(path):
        print('x 缺少 S 层快照：%s\n'
              '  它应该跟着仓库走（`switch_meanings.tsv`）。首次生成或源数据更新时跑：\n'
              '    python tools/gen_zh.py --refresh --write' % path, file=sys.stderr)
        return None, {}
    out = {}
    for line in open(path, encoding='utf-8'):
        line = line.rstrip('\n').rstrip('\r')
        if not line or line[0] == '#':
            continue
        f = line.split('\t')
        if len(f) != 5:
            print('x 快照行不是 5 列：%r' % line[:80], file=sys.stderr)
            return None, {}
        out[(unesc(f[0]), unesc(f[1]), unesc(f[2]).upper())] = (unesc(f[3]), f[4])
    dist = {}
    for _, src in out.values():
        dist[src] = dist.get(src, 0) + 1
    st = {'switch': len(out)}
    for k in ('manual', 'doc', 'vocab'):
        st['switch_' + k] = dist.get(k, 0)
    return out, st


def write_switch_snapshot(path, sw, stats):
    """把裁决结果写成仓内快照（**只在 `--refresh` 时调用**）。

    带三行注释头：说明、来源分布、三元组含义 —— 分布数字是确定性的（不含时间戳，
    这样同样的输入重跑两次字节相同）。
    """
    rows = sorted('%s\t%s\t%s\t%s\t%s' % (esc(m), esc(p), esc(n), esc(cn), src)
                  for (m, p, n), (cn, src) in sw.items())
    head = ('# 开关名（位名）中文的冻结快照 —— `tools/gen_zh.py --refresh` 生成，**勿手改**\n'
            '# 来源分布：manual %d / doc_desc %d / switch_vocab(n_spaces==1) %d = %d 条\n'
            '# 列：moc \\t param \\t switch_name \\t 中文 \\t src（三元组才能唯一定位，见 DESIGN D69）\n'
            % (stats.get('switch_manual', 0), stats.get('switch_doc', 0),
               stats.get('switch_vocab', 0), len(sw)))
    tmp = path + '.tmp'
    try:
        with open(tmp, 'w', encoding='utf-8', newline='\n') as f:
            f.write(head + '\n'.join(rows) + '\n')
        os.replace(tmp, path)
    except OSError as e:
        print('x 写快照失败：%s（%s）' % (path, e), file=sys.stderr)
        return None
    print('写入快照 %s（%d 行）' % (path, len(sw)))
    return len(sw)


def build_switch(docdb, manual, vocab, swdict, want):
    """合成开关名中文表：`{(moc,param,switch): (中文, src)}`。

    优先级 `manual` > `doc`（switch_meanings.doc_desc）> `vocab`（**仅 n_spaces==1**）。
    三元组全集 = 本仓 `Bits` 位名 ∪ `switch_dict` ∪ manual ∪ auto（只收全集里的键，
    否则会像 `SVR2` 那样被套到不相干的参数上）。多义位词与 `doc_option` 的英文一律不用。
    """
    man = {}
    if os.path.exists(manual):
        for e in json.load(open(manual, encoding='utf-8')):
            moc = str(e.get('moc', '')).upper()
            par = str(e.get('param', '')).upper()
            for name, v in (e.get('switches') or {}).items():
                if isinstance(v, (list, tuple)) and len(v) > 1 and has_cn(v[1]):
                    man[(moc, par, str(name).upper())] = str(v[1])
    else:
        print('x 缺少人工裁决源：%s' % manual, file=sys.stderr)
        return None, {}

    auto = {}
    if os.path.exists(docdb):
        con = sqlite3.connect('file:%s?mode=ro' % docdb.replace('\\', '/'), uri=True)
        try:
            for moc, par, name, desc in con.execute(
                    'SELECT moc, param_name, switch_name, doc_desc FROM switch_meanings'):
                if has_cn(desc):
                    auto[(str(moc).upper(), str(par).upper(), str(name).upper())] = str(desc)
        finally:
            con.close()
    else:
        print('x 缺少文档库：%s' % docdb, file=sys.stderr)
        return None, {}

    voc, multi = {}, 0
    if os.path.exists(vocab):
        for e in json.load(open(vocab, encoding='utf-8')):
            if e.get('means_src') == 'ascii' or not has_cn(e.get('means_cn')):
                continue
            n = e.get('n_spaces', len(e.get('spaces') or []))
            if n == 1:
                voc[e['bit'].upper()] = str(e['means_cn'])
            else:
                multi += 1
    else:
        print('x 缺少位词词典：%s' % vocab, file=sys.stderr)
        return None, {}

    universe = set(want) | switch_dict_triples(swdict) | set(man) | set(auto)
    out, n_man = {}, 0
    for k in universe:
        if k in man:
            out[k] = (man[k], 'manual')
            n_man += 1
    n_doc = n_voc = 0
    for k in universe:
        if k not in out and k in auto:
            out[k] = (auto[k], 'doc')
            n_doc += 1
    for k in universe:
        if k not in out and k[2] in voc:
            out[k] = (voc[k[2]], 'vocab')
            n_voc += 1
    covered_want = sum(1 for k in want if k in out)
    st = {'switch': len(out), 'switch_manual': n_man, 'switch_doc': n_doc,
          'switch_vocab': n_voc, 'switch_vocab_multi_skipped': multi,
          'want_bits': len(want), 'want_covered': covered_want}
    return out, st


def coverage():
    """对当前投影库算中文名覆盖率（只报，不参与退出码）。"""
    cur_db = None
    ptr = os.path.join(ROOT, 'temp', 'cur_db.txt')
    if os.path.exists(ptr):
        val = open(ptr, encoding='utf-8', errors='replace').read().strip()
        if val:
            cur_db = os.path.join(ROOT, val)
    if not cur_db or not os.path.exists(cur_db):
        print('（跳过覆盖率：没有投影库 temp\\cur_db.txt）')
        return
    con = sqlite3.connect('file:%s?mode=ro' % cur_db.replace('\\', '/'), uri=True)
    tabs = [x[0] for x in con.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'cur\\_%' ESCAPE '\\'")]
    total = hit = 0
    for t in tabs:
        moc = t[4:].upper()
        for c in [x[1] for x in con.execute('PRAGMA table_info("%s")' % t)]:
            total += 1
            if (moc, c.upper()) in P_SET or c.upper() in Q_SET:
                hit += 1
    print('覆盖：列 %d / 有中文名 %d（%.1f%%）' % (total, hit, 100.0 * hit / total if total else 0))
    con.close()


P_SET, Q_SET = set(), set()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--db', default=DEFAULT_DB, help='ims_asset/library.db（zh_* 5 张表）')
    ap.add_argument('--docdb', default=DEFAULT_DOCDB, help='agcf_mml.db（switch_meanings；仅 --refresh 用）')
    ap.add_argument('--manual', default=DEFAULT_MANUAL, help='manual_switch_meanings.json（仅 --refresh 用）')
    ap.add_argument('--vocab', default=DEFAULT_VOCAB, help='switch_vocab.json（仅 --refresh 用）')
    ap.add_argument('--swdict', default=DEFAULT_SWDICT, help='switch_dict.json（仅 --refresh 用）')
    ap.add_argument('--switch-tsv', default=DEFAULT_SWITCH_TSV,
                    help='S 层冻结快照（默认仓内 switch_meanings.tsv）')
    ap.add_argument('--refresh', action='store_true',
                    help='从 4 个外部输入重新裁决 S 层并**重写快照**（离线时不要加）')
    ap.add_argument('--diff', action='store_true',
                    help='只对比「外部输入重新裁决的结果」与仓内快照的差异，**不写任何文件**'
                         '（外部源更新后先跑这个看会变什么）')
    ap.add_argument('--write', action='store_true')
    ap.add_argument('--check', action='store_true')
    a = ap.parse_args()

    if a.diff:
        return cmd_diff(a.docdb, a.manual, a.vocab, a.swdict, a.switch_tsv)

    if a.refresh:
        print('!! --refresh：将从外部输入（library.db / agcf_mml.db / manual json / vocab json）'
              '重新裁决 S 层并覆盖 %s' % os.path.basename(a.switch_tsv))

    got = build(a.db, a.docdb, a.manual, a.vocab, a.swdict, a.switch_tsv, a.refresh)
    if got is None:
        return 4
    text, stats = got

    global P_SET, Q_SET
    for ln in text.splitlines():
        f = ln.split('\t')
        if len(f) < 3:
            continue
        if f[0] == 'P':
            P_SET.add((unesc(f[1]), unesc(f[2])))
        elif f[0] == 'Q':
            Q_SET.add(unesc(f[1]))

    print('zh.tsv：%d 行（%s）' % (len(text.splitlines()) - 2,
                                 ' / '.join('%s %d' % (k, v) for k, v in stats.items())))

    if a.check:
        if not os.path.exists(OUT):
            print('x --check：zh.tsv 不存在', file=sys.stderr)
            return 3
        disk = open(OUT, encoding='utf-8', newline='').read()
        if disk.replace('\r\n', '\n') != text:
            print('x --check：重算结果与磁盘不一致', file=sys.stderr)
            for i, (x, y) in enumerate(zip(disk.replace('\r\n', '\n').splitlines(),
                                          text.splitlines())):
                if x != y:
                    print('  第 %d 行：磁盘 %r / 重算 %r' % (i + 1, x[:80], y[:80]), file=sys.stderr)
                    break
            return 3
        print('ok --check：重算 == 磁盘（%d 行）' % (len(text.splitlines()) - 2))
        coverage()
        return 0

    if a.write:
        with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
            f.write(text)
        print('写入 %s' % OUT)
    else:
        print('（未写文件；加 --write）')
    coverage()
    return 0


if __name__ == '__main__':
    sys.exit(main())



