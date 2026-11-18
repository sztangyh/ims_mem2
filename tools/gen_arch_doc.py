# -*- coding: utf-8 -*-
"""生成 `docs/ARCH.html`：项目代码的可视化地图。

**所有事实都从源码抽取**（文件、LOC、首行 `//!` 说明、Cargo 依赖、验收门段名），
不靠手写猜测；只有"关键不变量""常见任务索引"这类判断是人工写的。

用法：python tools/gen_arch_doc.py [--out docs/ARCH.html]
"""
import argparse
import html
import os
import re
import string
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

CRATE_ROLE = {
    'core': ('存储引擎', '内存为唯一真源：表、列值、版本链、导入、索引、中文词典、MGW'),
    'macros': ('声明宏', '`mml_table!`：瘦声明 → Row/Pending/ColStore/TableHooks'),
    'proj': ('只读投影', '把内存导出成 SQLite（`cur_*` / `mgw_*` / `hist_*` / `zh_*`）'),
    'svc': ('服务端', 'Engine：对象组装 + 目录池 + 引用图 + JSONL/UDP 协议'),
    'tui': ('终端客户端', 'ratatui UI；默认经 UDP 连 svc，也可 inline 直连'),
    'web': ('Web 客户端', '静态 SPA 托管 + HTTP→JSONL 转发（默认 UDP）'),
}

LAYERS = [
    ('入口 / 导入', 'core::import · core::scan · core::mgw', '把 GBK 后备文件解析成表行'),
    ('存储引擎', 'core::table · vhist · sym · key_index · refs', '4 字节列值 + 列级版本链 + 索引'),
    ('服务 / 投影', 'svc::engine · proto · serve · proj', '组装对象、导出 SQLite、对外协议'),
    ('客户端', 'tui · web', '终端 UI、Web SPA（都经 UDP + AEAD）'),
]


def read(p):
    # PowerShell 的 Tee-Object 可能写 UTF-16LE，所以要多试几种编码
    for enc in ('utf-8', 'utf-16', 'utf-16-le', 'gbk'):
        try:
            return open(p, encoding=enc).read()
        except Exception:
            continue
    return ''


def first_doc(txt):
    """取文件头部第一行有意义的 `//!`。"""
    for line in txt.split('\n')[:40]:
        s = line.strip()
        if s.startswith('//!'):
            body = s[3:].strip()
            if body and not body.startswith('```') and not body.startswith('#'):
                return body
    return ''


def loc(txt):
    return sum(1 for _ in txt.split('\n'))


def collect():
    crates = {}
    for c in sorted(os.listdir(os.path.join(ROOT, 'crates'))):
        d = os.path.join(ROOT, 'crates', c)
        if not os.path.isdir(d):
            continue
        files = []
        for base, _dirs, names in os.walk(d):
            if 'target' in base:
                continue
            for n in sorted(names):
                if not n.endswith('.rs'):
                    continue
                p = os.path.join(base, n)
                txt = read(p)
                rel = os.path.relpath(p, d).replace('\\', '/')
                kind = 'example' if '/examples/' in '/' + rel else ('test' if '/tests/' in '/' + rel else 'src')
                files.append({
                    'rel': rel, 'kind': kind, 'loc': loc(txt),
                    'doc': first_doc(txt) or '（无头部说明）',
                })
        toml = read(os.path.join(d, 'Cargo.toml'))
        deps = re.findall(r'^([a-zA-Z0-9_\-]+)\s*=\s*\{[^}]*path\s*=\s*"\.\./([a-zA-Z0-9_\-]+)"',
                          toml, re.M)
        ws = re.findall(r'^([a-zA-Z0-9_\-]+)\s*=\s*"', toml, re.M)
        files.sort(key=lambda x: (x['kind'] != 'src', x['rel']))
        crates[c] = {
            'files': files,
            'path_deps': sorted({b for _a, b in deps}),
            'ext_deps': sorted(set(ws) - {'version', 'edition', 'name'}),
            'loc': sum(f['loc'] for f in files),
        }
    return crates


def gate_sections():
    """验收门的段名。

    优先用**最近一次门运行日志**里的真实段名（多数段是循环打印的，脚本里只有少数字面量），
    拿不到日志才退回脚本里的 `Write-Host '== … =='`。
    """
    import glob
    best = []
    for p in sorted(glob.glob(os.path.join(ROOT, 'temp', 'gate_run*.log'))):
        names = re.findall(r'^==\s*(.+?)\s*==\s*$', read(p), re.M)
        if len(names) > len(best):
            best = names
    if best:
        return best
    got = re.findall(r"Write-Host\s+['\"]==\s+(.+?)\s+==['\"]", read(os.path.join(ROOT, 'tools', 'check_tables.ps1')))
    return [g.strip() for g in got]


def tests_count():
    n = 0
    for base, dirs, names in os.walk(os.path.join(ROOT, 'crates')):
        if 'target' in base:
            continue
        for f in names:
            if f.endswith('.rs'):
                n += len(re.findall(r'#\[test\]', read(os.path.join(base, f))))
    return n


TPL = string.Template('''<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>ims_mem2 · 代码地图</title>
<style>
:root{--bg:#f6f7f9;--panel:#fff;--ink:#16181d;--ink2:#4a5160;--ink3:#8b93a3;--line:#e6e8ee;
--brand:#3b5bdb;--soft:#eef1fe;--ok:#127a4b;--oks:#e7f7ef;--warn:#9a6a00;--warns:#fdf3dd;
--mono:ui-monospace,SFMono-Regular,Consolas,monospace}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.6 -apple-system,"Segoe UI","Microsoft YaHei",system-ui,sans-serif}
.wrap{max-width:1080px;margin:0 auto;padding:34px 22px 80px}
h1{font-size:26px;margin:0 0 4px}
h2{font-size:18px;margin:38px 0 12px;padding-bottom:6px;border-bottom:1px solid var(--line)}
h3{font-size:14px;margin:20px 0 8px;color:var(--ink2)}
.sub{color:var(--ink3);font-size:13px;margin:0 0 6px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:16px 18px;margin:12px 0;
box-shadow:0 1px 2px rgba(16,20,30,.05)}
code,.mono{font-family:var(--mono);font-size:12.5px}
.pill{display:inline-block;font-size:11.5px;padding:1px 8px;border-radius:99px;background:var(--soft);color:var(--brand);margin-right:6px}
.pill.g{background:var(--oks);color:var(--ok)}
.pill.w{background:var(--warns);color:var(--warn)}
table{width:100%;border-collapse:collapse;font-size:13px}
th{text-align:left;font-size:11px;letter-spacing:.4px;text-transform:uppercase;color:var(--ink3);
padding:7px 10px;border-bottom:1px solid var(--line);background:var(--panel)}
td{padding:7px 10px;border-bottom:1px solid #eef0f4;vertical-align:top}
tr:last-child td{border-bottom:0}
td.n{text-align:right;font-family:var(--mono);color:var(--ink3);white-space:nowrap}
.loc{color:var(--ink3);font-family:var(--mono);font-size:12px}
.role{font-size:12px;color:var(--ink3);margin:2px 0 10px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:12px}
.kv{font-size:13px}
.kv b{color:var(--ink)}
ul{margin:8px 0;padding-left:20px}
ul li{margin:3px 0}
.leaf{background:var(--panel);border:1px solid var(--line);border-radius:10px;padding:10px 12px;font-size:12.5px}
.leaf .t{font-weight:600;font-size:13px;margin-bottom:2px}
.flow{display:flex;flex-direction:column;gap:0;align-items:stretch}
.step{display:flex;gap:12px;align-items:flex-start;background:var(--panel);border:1px solid var(--line);
border-radius:10px;padding:12px 14px}
.step .n{width:24px;height:24px;flex:none;display:grid;place-items:center;border-radius:7px;
background:var(--brand);color:#fff;font-size:12px;font-weight:700}
.step .b{flex:1}
.step .b b{display:block;font-size:13.5px;margin-bottom:2px}
.step .b span{font-size:12.5px;color:var(--ink3)}
.arrow{align-self:center;color:var(--ink3);font-size:16px;line-height:1;padding:3px 0}
footer{margin-top:50px;color:var(--ink3);font-size:12px;text-align:center}
</style></head><body><div class="wrap">

<h1>ims_mem2 · 代码地图</h1>
<p class="sub">从源码自动抽取（$stamp）· $ncrate 个 crate · $nfiles 个 .rs · $nloc 行 · $ntest 个 <code>#[test]</code></p>

<h2>1. 四层结构</h2>
<p class="sub">数据只往一个方向流：磁盘 → 内存表 → 投影/服务 → 界面。内存是唯一真源，SQLite 只读。</p>
<div class="flow">$layers</div>

<h2>2. 数据从磁盘到界面的五步</h2>
<div class="flow">$steps</div>

<h2>3. crate 与依赖</h2>
<div class="grid">$crate_cards</div>
<div class="card"><table><thead><tr><th>crate</th><th class="n">行数</th><th>依赖（本仓）</th></tr></thead><tbody>
$dep_rows
</tbody></table></div>

<h2>4. 每个文件做什么</h2>
<p class="sub">「说明」列取自文件头部的 <code>//!</code> —— 改了代码就顺手改它，这张表才不会骗人。</p>
$modules

<h2>5. 关键约定（改代码前必须知道）</h2>
<div class="card kv"><ul>
<li><b>列值 4 字节</b>：<code>ColVal = 2 位 tag + 30 位 payload</code>（<code>Null/Text/Num/Enum/Bool/Bits</code>）。
文本/枚举不存指针，存 <code>Interner</code>/<code>LabelCols</code> 的索引。</li>
<li><b>内存是唯一真源</b>：SQLite 投影只写不读，导入后整库重算（D1）。</li>
<li><b>SCD2 版本链</b>：每个<b>列</b>一条版本链（不是每行）——只记录真正变过的列。</li>
<li><b>稳定行 id</b>：<code>(tbl, row)</code>，行号不改；删除是墓碑（<code>tombs</code>），不是移动。</li>
<li><b>编码</b>：值按 GBK 原字节驻留，导出/报告时才转码；<b>中文词典用独立 UTF-8 池</b>（两套不能混）。</li>
<li><b>批量语义</b>：<code>set_batch</code> 切批、<code>set_snapshot(true)</code> 让"本批没出现 = 删除"。</li>
<li><b>查不到就空着</b>：不猜、不兜底（中文名、引用、缺省值都一样）。</li>
<li><b>验收门</b>：<code>tools/check_tables.ps1</code> 共 $nsect 段；改完必须跑，看退出码而不是看有没有 <code>--&gt;</code>。</li>
</ul></div>

<h2>6. 验收门 $nsect 段</h2>
<div class="card"><table><thead><tr><th class="n">#</th><th>段</th></tr></thead><tbody>
$gate_rows
</tbody></table></div>

<h2>7. 想改什么，看哪里</h2>
<div class="card"><table><thead><tr><th>目标</th><th>入口</th></tr></thead><tbody>
<tr><td>加一张表 / 改列</td><td><code>crates/core/src/*_gen.rs</code> 或 <code>schema_gen_data.rs</code>（生成器：<code>tools/port_schema.py</code>）</td></tr>
<tr><td>改导入解析</td><td><code>crates/core/src/scan.rs</code> · <code>import.rs</code></td></tr>
<tr><td>改存储/版本链</td><td><code>table.rs</code>（引擎）· <code>vhist.rs</code>（列版本）</td></tr>
<tr><td>加/改投影</td><td><code>crates/proj/src/lib.rs</code>（cur/hist/zh）· <code>mgw.rs</code>（MGW）</td></tr>
<tr><td>改对象/链接组装</td><td><code>crates/svc/src/engine.rs</code></td></tr>
<tr><td>改协议（JSONL）</td><td><code>crates/svc/src/proto.rs</code>（<code>dispatch</code> 是唯一入口）</td></tr>
<tr><td>改 UDP/加密/分片</td><td><code>crates/core/src/wire.rs</code> · <code>crates/svc/src/serve.rs</code></td></tr>
<tr><td>改终端界面</td><td><code>crates/tui/src/main.rs</code>（UI）· <code>proto.rs</code>（解析）</td></tr>
<tr><td>改网页</td><td><code>crates/web/src/static/{index.html,app.js,style.css}</code></td></tr>
<tr><td>改验收</td><td><code>tools/check_tables.ps1</code> + 各 <code>tools/check_*.py</code></td></tr>
</tbody></table></div>

<footer>由 <code>tools/gen_arch_doc.py</code> 生成 · 重新生成：<code>python -X utf8 tools/gen_arch_doc.py</code></footer>
</div></body></html>
''')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', default=os.path.join(ROOT, 'docs', 'ARCH.html'))
    a = ap.parse_args()

    crates = collect()
    gates = gate_sections()
    ntest = tests_count()
    nfiles = sum(len(c['files']) for c in crates.values())
    nloc = sum(c['loc'] for c in crates.values())

    e = html.escape
    layers = []
    for i, (name, mods, desc) in enumerate(LAYERS):
        layers.append('<div class="step"><div class="n">%d</div><div class="b"><b>%s</b>'
                      '<span>%s<br><code>%s</code></span></div></div>' % (i + 1, e(name), e(desc), e(mods)))
        if i + 1 < len(LAYERS):
            layers.append('<div class="arrow">▼</div>')

    steps = [
        ('解析（GBK 原字节）', 'scan.rs / mgw::parse_log 按 CGP/MOC/报表切段，产出 MML 行的事件流'),
        ('写表', 'Table::apply → Pending → 键索引 + 稳定 row id；墓碑标记删除'),
        ('记版本', 'vhist 只给**变过的列**追加 (row, col, batch) 记录（SCD2）'),
        ('投影', 'proj 把内存整库重算成 SQLite：cur_* 当前值 / hist_* 历史 / mgw_* / zh_*'),
        ('服务', 'svc::Engine 把表行组装成对象（字段 + 链接 + 中文名），按 io_spec 对外'),
        ('界面', 'tui / web 通过 JSONL over UDP(+AEAD) 查询；也可 inline 直连做自检'),
    ]
    step_html = []
    for i, (t, d) in enumerate(steps):
        step_html.append('<div class="step"><div class="n">%d</div><div class="b"><b>%s</b>'
                         '<span>%s</span></div></div>' % (i + 1, e(t), e(d)))
        if i + 1 < len(steps):
            step_html.append('<div class="arrow">▼</div>')

    cards = []
    for c, info in crates.items():
        role, desc = CRATE_ROLE.get(c, ('', ''))
        cards.append('<div class="leaf"><div class="t">%s <span class="pill">%s</span></div>'
                     '<div class="role">%s</div><div class="loc">%d 行 · %d 个 .rs</div></div>'
                     % (e(c), e(role), e(desc), info['loc'], len(info['files'])))

    dep_rows = []
    for c, info in crates.items():
        pd = ', '.join('<code>%s</code>' % e(x) for x in info['path_deps']) or '—'
        dep_rows.append('<tr><td><code>%s</code></td><td class="n">%d</td><td>%s</td></tr>'
                        % (e(c), info['loc'], pd))

    mods = []
    for c, info in crates.items():
        role, desc = CRATE_ROLE.get(c, ('', ''))
        rows = []
        for f in info['files']:
            cls = {'src': '', 'example': ' <span class="pill w">example</span>',
                   'test': ' <span class="pill g">test</span>'}[f['kind']]
            rows.append('<tr><td><code>%s</code>%s</td><td class="n">%d</td><td>%s</td></tr>'
                        % (e(f['rel']), cls, f['loc'], e(f['doc'])))
        mods.append('<h3>%s <span class="pill">%s</span> <span class="loc">%d 行</span></h3>'
                    '<div class="role">%s</div><div class="card"><table><thead><tr>'
                    '<th>文件</th><th class="n">行</th><th>说明</th></tr></thead><tbody>%s</tbody></table></div>'
                    % (e(c), e(role), info['loc'], e(desc), ''.join(rows)))

    gate_rows = ''.join('<tr><td class="n">%d</td><td>%s</td></tr>' % (i + 1, e(g))
                        for i, g in enumerate(gates))

    out = TPL.safe_substitute(
        stamp='源码抽取',
        ncrate=len(crates), nfiles=nfiles, nloc=nloc, ntest=ntest,
        nsect=len(gates),
        layers=''.join(layers), steps=''.join(step_html),
        crate_cards=''.join(cards), dep_rows=''.join(dep_rows),
        modules=''.join(mods), gate_rows=gate_rows,
    )
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    with open(a.out, 'w', encoding='utf-8', newline='\n') as f:
        f.write(out)
    print('写入 %s（%.1f KB）' % (a.out, len(out.encode('utf-8')) / 1024))
    print('crate %d / 文件 %d / 行 %d / 测试 %d / 门段 %d'
          % (len(crates), nfiles, nloc, ntest, len(gates)))
    for c, info in crates.items():
        print('  %-8s %6d 行  %2d 文件  依赖 %s' % (c, info['loc'], len(info['files']),
                                                ','.join(info['path_deps']) or '-'))


if __name__ == '__main__':
    sys.exit(main())
