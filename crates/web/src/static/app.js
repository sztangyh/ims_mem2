/* =====================================================================
 * IMS 内存库 · SPA 客户端
 *
 * 路由（hash，可深链、可前进后退）：
 *   #/search                       查询页（默认）
 *   #/search/<pool>/<kind>/<arg>   直接带查询条件（刷新/分享都保留）
 *   #/obj/<io>/<ne>/<key>          对象详情（key 做 URL 编码）
 *   #/history                      历史（全部）
 *   #/help                         说明
 *
 * 缓存：`ims.cache`（localStorage，TTL 10 分钟，最多 200 条）+ 内存 Map。
 *       命中时右上角显示"缓存"角标；清缓存按钮在侧栏。
 * 历史：`ims.history`（最多 80 条，按 (io,ne,key) 去重，最近的排前面）。
 * ===================================================================== */
'use strict';

/* ---------------- 小工具 ---------------- */
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
const esc = (s) => String(s == null ? '' : s)
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
const enc = encodeURIComponent;
const dec = (s) => { try { return decodeURIComponent(s); } catch (e) { return s; } };

const LS = {
  get(k, d) { try { const v = localStorage.getItem(k); return v == null ? d : JSON.parse(v); } catch (e) { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch (e) {} },
  del(k) { try { localStorage.removeItem(k); } catch (e) {} },
};

/* ---------------- 缓存（带 TTL + LRU 裁剪） ---------------- */
const CACHE = {
  TTL: 10 * 60 * 1000,
  MAX: 200,
  mem: new Map(),
  load() { const o = LS.get('ims.cache', {}); this.mem = new Map(Object.entries(o)); },
  save() {
    const now = Date.now();
    let es = Array.from(this.mem.entries())
      .filter(([, v]) => now - v.t < this.TTL)
      .sort((a, b) => b[1].t - a[1].t)
      .slice(0, this.MAX);
    this.mem = new Map(es);
    LS.set('ims.cache', Object.fromEntries(this.mem));
  },
  get(url) {
    const v = this.mem.get(url);
    if (!v) return null;
    if (Date.now() - v.t > this.TTL) { this.mem.delete(url); return null; }
    return v.d;
  },
  put(url, d) { this.mem.set(url, { t: Date.now(), d }); this.save(); },
  clear() { this.mem.clear(); LS.del('ims.cache'); },
  size() { return this.mem.size; },
};

/* ---------------- 历史 ---------------- */
const HISTORY = {
  MAX: 80,
  list() { return LS.get('ims.history', []); },
  push(e) {
    const l = this.list().filter((x) => !(x.io === e.io && x.key === e.key && x.ne === e.ne));
    l.unshift({ io: e.io, ne: e.ne, key: e.key, label: e.label || e.key, ts: Date.now() });
    LS.set('ims.history', l.slice(0, this.MAX));
    renderHistory();
  },
  clear() { LS.del('ims.history'); renderHistory(); },
};

/* ---------------- 接口 ---------------- */
const API = (path, params) => {
  const qs = new URLSearchParams(params).toString();
  const url = path + (qs ? '?' + qs : '');
  const hit = CACHE.get(url);
  if (hit) return Promise.resolve({ ...hit, __cache: true });
  return fetch(url).then((r) => r.json()).then((d) => {
    if (d && d.ok) CACHE.put(url, d);
    return d;
  });
};

/* ---------------- toast ---------------- */
function toast(msg, kind) {
  const box = $('#toast');
  const el = document.createElement('div');
  el.className = 'toast' + (kind ? ' ' + kind : '');
  el.textContent = msg;
  box.appendChild(el);
  setTimeout(() => el.remove(), 3200);
}

/* ---------------- 服务状态 ---------------- */
async function pollStatus() {
  const el = $('#svc-status');
  const txt = el.querySelector('span');
  try {
    const d = await fetch('/api/hello').then((r) => r.json());
    el.className = 'status ok';
    txt.textContent = '服务正常' + (d && d.v ? ' · v' + d.v : '');
  } catch (e) {
    el.className = 'status bad';
    txt.textContent = '连不上 svc';
  }
}

/* ---------------- 主题 ---------------- */
function initTheme() {
  $('#theme-btn').addEventListener('click', () => {
    const cur = document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light';
    const next = cur === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = next;
    LS.set('ims.theme', next);
  });
  try { LS.set('ims.theme', document.documentElement.dataset.theme); } catch (e) {}
}

/* ---------------- 侧栏历史 ---------------- */
function renderHistory() {
  const l = HISTORY.list();
  $('#hist-count').textContent = l.length ? String(l.length) : '';
  const box = $('#hist-list');
  if (!l.length) { box.innerHTML = '<li class="empty">还没有浏览记录</li>'; return; }
  box.innerHTML = l.slice(0, 12).map((e) => {
    const href = '#/obj/' + enc(e.io) + '/' + e.ne + '/' + enc(e.key);
    return '<li><a href="' + href + '" title="' + esc(e.label) + '">' +
      '<span class="h-io">' + esc(e.io) + '</span>' + esc(e.label) + '</a></li>';
  }).join('');
}

/* ---------------- 通用渲染块 ---------------- */
const skeleton = (n = 5) =>
  '<div class="skel">' + Array.from({ length: n }, (_, i) =>
    '<i style="width:' + (92 - i * 11) + '%"></i>').join('') + '</div>';

const emptyState = (title, sub, code) =>
  '<div class="state"><div class="big">⌕</div><h3>' + esc(title) + '</h3>' +
  '<p>' + (sub || '') + (code ? ' 试试 <code>' + esc(code) + '</code>' : '') + '</p></div>';

function poolName(p) {
  return { prefix: '字冠', subscriber: '用户', trunk: '中继' }[p] || p;
}
function ioOfPool(p, item) {
  if (item && item.io) return item.io;
  return { prefix: 'PrefixProfile', subscriber: 'Subscriber', trunk: 'Trunk' }[p] || 'Subscriber';
}

/* ---------------- 视图：搜索 ---------------- */
let SEARCH = { pool: 'prefix', kind: 'prefix', arg: '', items: [], total: 0, after: null, loading: false };

function renderSearch() {
  const v = $('#view');
  v.innerHTML =
    '<div class="h-title"><h2>查询</h2><span class="meta">字冠 / 用户 / 中继 · 输入即查 · 点行看全貌</span></div>' +
    '<p class="sub">三池目录：字冠走 CNACLD/CLRDSN 等 7 类来源，用户按号码，中继按 TG。' +
    '支持 <kbd>Ctrl</kbd>+<kbd>K</kbd> 快速唤出搜索。</p>' +
    '<div class="card searchbar">' +
      '<div class="pool-tabs">' +
        ['prefix', 'subscriber', 'trunk'].map((p) =>
          '<button class="pool-tab' + (SEARCH.pool === p ? ' active' : '') + '" data-pool="' + p + '">' +
          poolName(p) + '</button>').join('') +
      '</div>' +
      '<div class="search-row">' +
        '<input id="q" type="text" spellcheck="false" autocomplete="off" ' +
          'placeholder="输入' + poolName(SEARCH.pool) + '，如 7552 / 82001 / 100" value="' + esc(SEARCH.arg) + '">' +
        '<select id="q-mode">' +
          '<option value="prefix"' + (SEARCH.kind === 'prefix' ? ' selected' : '') + '>前缀匹配</option>' +
          '<option value="tail"' + (SEARCH.kind === 'tail' ? ' selected' : '') + '>尾号匹配</option>' +
        '</select>' +
        '<button class="btn primary" id="q-go">查询</button>' +
      '</div>' +
      '<div class="hint" id="q-hint"></div>' +
    '</div>' +
    '<div id="results"></div>';

  $$('.pool-tab').forEach((b) => b.addEventListener('click', () => {
    SEARCH.pool = b.dataset.pool;
    SEARCH.arg = ''; SEARCH.items = []; SEARCH.total = 0; SEARCH.after = null;
    renderSearch();
    $('#q').focus();
  }));
  const input = $('#q');
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { SEARCH.arg = input.value.trim(); doSearch(); }
  });
  $('#q-mode').addEventListener('change', (e) => { SEARCH.kind = e.target.value; });
  $('#q-go').addEventListener('click', () => { SEARCH.arg = input.value.trim(); doSearch(); });
  if (SEARCH.arg) doSearch();
  else if (document.activeElement !== input) input.focus();
}

async function doSearch(more) {
  const box = $('#results');
  if (!SEARCH.arg) { box.innerHTML = ''; $('#q-hint').textContent = ''; return; }
  SEARCH.loading = true;
  if (!more) { SEARCH.items = []; SEARCH.after = null; }
  box.innerHTML = '<div class="card card-pad">' + skeleton(6) + '</div>';
  $('#q-hint').textContent = '查询中…';
  const params = { pool: SEARCH.pool, kind: SEARCH.kind, arg: SEARCH.arg, limit: 50 };
  if (more && SEARCH.after) params.after = SEARCH.after;
  try {
    const d = await API('/api/catalog', params);
    SEARCH.loading = false;
    if (!d || !d.ok) {
      box.innerHTML = emptyState('查询失败', (d && d.error && d.error.message) || '服务返回错误');
      $('#q-hint').textContent = '';
      return;
    }
    SEARCH.total = d.total || 0;
    const items = d.items || [];
    SEARCH.items = more ? SEARCH.items.concat(items) : items;
    SEARCH.after = d.after || null;
    $('#q-hint').innerHTML = '共 <b>' + SEARCH.total + '</b> 条 · 已显示 ' + SEARCH.items.length +
      (d.__cache ? ' · <span class="tag ok">缓存</span>' : '');
    renderResults();
    // 深链：把查询条件写进 URL（不触发路由重渲染）
    history.replaceState(null, '', '#/search/' + SEARCH.pool + '/' + SEARCH.kind + '/' + enc(SEARCH.arg));
  } catch (e) {
    SEARCH.loading = false;
    box.innerHTML = emptyState('连不上服务', esc(String(e)));
    $('#q-hint').textContent = '';
  }
}

function renderResults() {
  const box = $('#results');
  if (!SEARCH.items.length) {
    box.innerHTML = '<div class="card">' + emptyState('没有命中', '换个前缀试试，或用 <kbd>Ctrl</kbd>+<kbd>K</kbd> 换池。', SEARCH.arg) + '</div>';
    return;
  }
  const rows = SEARCH.items.map((it, i) => {
    const key = it.key != null ? it.key : (it.k != null ? it.k : '');
    const core = it.core ? '<div class="core">' + esc(it.core) + '</div>' : '';
    const roles = (it.roles || []).map((r) => '<span class="tag brand">' + esc(r) + '</span>').join('');
    const nes = (it.nes || []).map((n) => '<span class="tag">NE ' + esc(n) + '</span>').join('');
    const land = it.lands === true ? '<span class="tag ok">落地</span>' : '';
    const hits = it.hits != null ? '<span class="muted">' + esc(it.hits) + ' 行</span>' : '';
    return '<tr data-i="' + i + '"><td class="k">' + esc(key) + core + '</td>' +
      '<td>' + roles + land + '</td><td>' + nes + '</td><td>' + hits + '</td></tr>';
  }).join('');
  const more = SEARCH.total > SEARCH.items.length
    ? '<div style="padding:12px;text-align:center"><button class="btn" id="more">加载更多</button></div>' : '';
  box.innerHTML = '<div class="card"><div class="table-wrap"><table class="res">' +
    '<thead><tr><th>键</th><th>角色</th><th>网元</th><th>规模</th></tr></thead>' +
    '<tbody>' + rows + '</tbody></table></div>' + more + '</div>';
  $$('table.res tbody tr').forEach((tr) => tr.addEventListener('click', () => {
    const it = SEARCH.items[+tr.dataset.i];
    const key = it.key != null ? it.key : it.k;
    const ne = (it.nes && it.nes[0]) || 1;
    location.hash = '#/obj/' + enc(ioOfPool(SEARCH.pool, it)) + '/' + ne + '/' + enc(key);
  }));
  if ($('#more')) $('#more').addEventListener('click', () => doSearch(true));
}

/* ---------------- 视图：对象详情 ---------------- */
async function renderObj(io, ne, key) {
  const v = $('#view');
  v.innerHTML =
    '<div class="h-title"><h2>对象详情</h2><span class="meta">' + esc(io) + '</span></div>' +
    '<div class="obj-head"><span class="chip">' + esc(io) + '</span>' +
      '<span class="chip">ne ' + esc(ne) + '</span><span class="chip">' + esc(key) + '</span></div>' +
    '<div id="obj-body"><div class="card card-pad">' + skeleton(7) + '</div></div>';
  let d;
  try {
    d = await API('/api/object', { io: io, ne: ne, key: key });
  } catch (e) {
    $('#obj-body').innerHTML = '<div class="card">' + emptyState('连不上服务', esc(String(e))) + '</div>';
    return;
  }
  const body = $('#obj-body');
  if (!d || !d.ok) {
    const m = (d && d.error && d.error.message) || 'object not found';
    body.innerHTML = '<div class="card">' + emptyState('没有这个对象', esc(m),
      io === 'PrefixProfile' ? '7552' : '') + '</div>';
    return;
  }
  HISTORY.push({ io: io, ne: ne, key: key, label: d.key || key });

  if (d.detail) { body.innerHTML = renderDetail(d.detail, d); return; }

  const fields = (d.fields || []).map(([k, val, cn]) =>
    '<div><span class="f-name">' + esc(k) + '</span>' +
      (cn ? '<span class="f-cn">' + esc(cn) + '</span>' : '') + '</div>' +
    '<div class="f-val' + (val == null || val === '' ? ' empty' : '') + '">' +
      (val == null || val === '' ? '<i>（空）</i>' : esc(val)) + '</div>').join('');

  const bits = (d.bits || []).map((b) => {
    const items = (b.items || []).map(([n, on, cn]) =>
      '<span class="bit' + (on ? ' on' : '') + '">' + esc(n) +
      (cn ? '<small>' + esc(cn) + '</small>' : '') + '</span>').join('');
    return '<div class="sect"><h3>开关 · ' + esc(b.col) + '</h3><div class="bit-grid">' + items + '</div></div>';
  }).join('');

  const links = (d.links || []).map(([f, card, n]) =>
    '<span class="tag">' + esc(f) + ' ' + esc(card) + ' · ' + esc(n) + '</span>').join('');

  body.innerHTML = '<div class="card"><div class="fields">' + fields + '</div></div>' +
    (bits ? '<div class="card card-pad">' + bits + '</div>' : '') +
    (links ? '<div class="sect"><h3>链接</h3><div>' + links + '</div></div>' : '');
}

/* 结构化的 PrefixProfile 详情（落地窗等）—— 尽量通用地渲染 */
function renderDetail(d, o) {
  const kv = (k, v) => '<div><span class="f-name">' + esc(k) + '</span></div><div class="f-val">' +
    (v == null ? '<i>（空）</i>' : esc(typeof v === 'object' ? JSON.stringify(v) : v)) + '</div>';
  let html = '';
  if (d.overview || d.总览) html += '<div class="card card-pad" style="margin-bottom:14px">' +
    esc(d.overview || d.总览) + '</div>';
  for (const [k, v] of Object.entries(d)) {
    if (k === 'overview' || k === '总览') continue;
    if (Array.isArray(v) && v.length && typeof v[0] === 'object') {
      const rows = v.map((row) => '<tr>' + Object.values(row).map((x) =>
        '<td class="f-val">' + esc(x == null ? '' : x) + '</td>').join('') + '</tr>').join('');
      const head = Object.keys(v[0]).map((h) => '<th>' + esc(h) + '</th>').join('');
      html += '<div class="sect"><h3>' + esc(k) + ' · ' + v.length + '</h3>' +
        '<div class="card"><div class="table-wrap"><table class="res"><thead><tr>' + head +
        '</tr></thead><tbody>' + rows + '</tbody></table></div></div></div>';
    } else if (v != null && typeof v === 'object' && !Array.isArray(v)) {
      html += '<div class="sect"><h3>' + esc(k) + '</h3><div class="card"><div class="fields">' +
        Object.entries(v).map(([a, b]) => kv(a, b)).join('') + '</div></div></div>';
    } else {
      html += '<div class="sect"><h3>' + esc(k) + '</h3><div class="card"><div class="fields">' +
        kv(k, v) + '</div></div></div>';
    }
  }
  return html || '<div class="card card-pad muted">（空详情）</div>';
}

/* ---------------- 视图：历史 ---------------- */
function renderHistoryView() {
  const l = HISTORY.list();
  const v = $('#view');
  v.innerHTML =
    '<div class="h-title"><h2>历史</h2><span class="meta">' + l.length + ' 条 · 只存在本机 localStorage</span></div>' +
    '<p class="sub">点任意一条回到那个对象；<b>清空</b>不影响缓存。</p>' +
    (l.length
      ? '<div class="card"><div class="table-wrap"><table class="res"><thead><tr>' +
        '<th>对象</th><th>IO</th><th>键</th><th>时间</th></tr></thead><tbody>' +
        l.map((e) => {
          const href = '#/obj/' + enc(e.io) + '/' + e.ne + '/' + enc(e.key);
          const t = new Date(e.ts);
          return '<tr data-href="' + href + '"><td>' + esc(e.label) + '</td>' +
            '<td><span class="tag brand">' + esc(e.io) + '</span></td>' +
            '<td class="k">' + esc(e.key) + '</td>' +
            '<td class="muted">' + t.toLocaleString('zh-CN', { hour12: false }) + '</td></tr>';
        }).join('') + '</tbody></table></div></div>'
      : '<div class="card">' + emptyState('还没有浏览记录', '去查询页点一行试试。') + '</div>');
  $$('tr[data-href]').forEach((tr) => tr.addEventListener('click', () => { location.hash = tr.dataset.href; }));
}

/* ---------------- 视图：说明 ---------------- */
function renderHelp() {
  $('#view').innerHTML =
    '<div class="h-title"><h2>说明</h2><span class="meta">语义 · 快捷键 · 缓存</span></div>' +
    '<div class="card card-pad prose">' +
      '<h3>查询语义</h3><ul>' +
        '<li><b>字冠池</b>：被叫 CNACLD、主叫甄别 CLRDSN、中继缺省主叫等 7 类来源的 TG 并集；' +
          '自动对称剥冠（0086755 / 86755 / 0755 / 755 / 0086 / 86 / 0）。</li>' +
        '<li><b>用户池</b>：订阅用户号码；前缀匹配，切到尾号可查本地短号。</li>' +
        '<li><b>中继池</b>：PRA / No7 / SIP 中继群号。</li>' +
        '<li><b>点行看全貌</b>：字冠显示被叫/主叫甄别/缺省主叫 + <b>落地窗</b>（RSC → 路由 → 子路由 → 出局中继）。</li>' +
      '</ul>' +
      '<h3>MGW（媒体网关）</h3><ul>' +
        '<li><b>Mgw</b>：一台 MGW 一个对象，键 = 简称（<code>lh1</code>）或 <code>IP:port</code>。</li>' +
        '<li><b>MgwLink</b>：带 IP 的链路，键 = <code>网元/族/链路</code>，如 <code>lh1/l2ua/IUA/0</code>。</li>' +
        '<li><b>Q921Link</b> / <b>V5Link</b> / <b>Tdmiu</b> / <b>Board</b>：键用 <code>/</code> 连自然键，' +
          '如 <code>lh1/1/0/FRONT</code>（框/槽/前后插）。</li>' +
        '<li>MGW 的中文名来自独立的 <code>mgw_zh.tsv</code>（770 条）——与 AGCF 词典分开，因为两边参数同名不同义。</li>' +
      '</ul>' +
      '<h3>快捷键</h3><table class="kbd-table">' +
        '<tr><td><kbd>/</kbd> 或 <kbd>Ctrl</kbd>+<kbd>K</kbd></td><td>唤出搜索</td></tr>' +
        '<tr><td><kbd>Enter</kbd></td><td>搜索框内直接查询</td></tr>' +
        '<tr><td><kbd>Esc</kbd></td><td>关闭搜索面板</td></tr>' +
        '<tr><td><kbd>Alt</kbd>+<kbd>←</kbd></td><td>浏览器后退（SPA 路由，不会整页刷新）</td></tr>' +
      '</table>' +
      '<h3>缓存与历史</h3><ul>' +
        '<li>查询结果缓存 <b>10 分钟</b>（本机 localStorage，最多 200 条）；命中时提示条会显示「缓存」角标。</li>' +
        '<li>侧栏「清缓存」立即失效全部缓存；「历史」是浏览记录，两者互不影响。</li>' +
        '<li>地址栏是状态：查询条件、对象详情都能收藏/分享，刷新不丢。</li>' +
      '</ul>' +
    '</div>';
}

/* ---------------- 路由 ---------------- */
function setNav(name) {
  $$('.nav a').forEach((a) => a.classList.toggle('active', a.dataset.nav === name));
}

function route() {
  const h = location.hash.replace(/^#\/?/, '');
  const seg = h.split('/').filter((x) => x.length);
  const kind = seg[0] || 'search';
  if (kind === 'obj' && seg.length >= 4) {
    setNav('');
    renderObj(dec(seg[1]), seg[2], dec(seg.slice(3).join('/')));
    return;
  }
  if (kind === 'history') { setNav('history'); renderHistoryView(); return; }
  if (kind === 'help') { setNav('help'); renderHelp(); return; }
  setNav('search');
  if (kind === 'search' && seg.length >= 4) {
    SEARCH.pool = seg[1] || 'prefix';
    SEARCH.kind = seg[2] || 'prefix';
    SEARCH.arg = dec(seg.slice(3).join('/'));
    SEARCH.items = []; SEARCH.total = 0; SEARCH.after = null;
  }
  renderSearch();
}

/* ---------------- 命令面板 ---------------- */
function initPalette() {
  const wrap = $('#palette');
  const input = $('#palette-input');
  const open = () => { wrap.hidden = false; input.value = ''; input.focus(); };
  const close = () => { wrap.hidden = true; };
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      const v = input.value.trim();
      close();
      if (v) { location.hash = '#/search/prefix/prefix/' + enc(v); }
    } else if (e.key === 'Escape') close();
  });
  wrap.addEventListener('click', (e) => { if (e.target === wrap) close(); });
  document.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); open(); return; }
    const tag = (document.activeElement && document.activeElement.tagName) || '';
    if (e.key === '/' && tag !== 'INPUT' && tag !== 'SELECT' && tag !== 'TEXTAREA') {
      e.preventDefault(); open();
    }
    if (e.key === 'Escape' && !wrap.hidden) close();
  });
}

/* ---------------- 启动 ---------------- */
function boot() {
  CACHE.load();
  initTheme();
  initPalette();
  renderHistory();
  $('#clear-hist').addEventListener('click', () => { HISTORY.clear(); toast('历史已清空', 'ok'); });
  $('#clear-cache').addEventListener('click', () => {
    CACHE.clear();
    toast('缓存已清空（当前 0 条）', 'ok');
  });
  pollStatus();
  setInterval(pollStatus, 20000);
  window.addEventListener('hashchange', route);
  route();
}

document.addEventListener('DOMContentLoaded', boot);
