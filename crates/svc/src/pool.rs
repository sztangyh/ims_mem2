//! fzf 目录池（服务端物化，`spec/num_index.md` 定案）：
//!
//! - **对称剥冠**（§3.5）：冠集 P 长优先；过程态（正在打冠）原样；其余剥最长冠得**裸核心**。
//!   索引端为每个原字冠建 `(核心, 原文)` 与 `(原文, 原文)` 双条目；查询端同一函数剥输入。
//!   **客户端展示永远是原字冠**（命中核心键 → 展开该核心的全部原文形态，同核心相邻）。
//! - **落地标志**（决议 4）：构建期沿 `pfx→rsc→rtana→rt(sr 全宽 28)→srt.o→ofc.dol=LOW` 算好。
//! - **用户号码池**：D63a 排列数组（只存行号，不复制键串）——前缀正排 + 尾号逆排。
//!
//! 预算（D63：≤40 B/键、≤正文 10%）：字冠池 ≈3 MB、用户池 2×2.7 MB，合计 ≈8 MB。

use std::collections::{HashMap, HashSet};

use ims_mem2::table::TableApi;
use ims_mem2::trunk::FAMILIES;
use ims_mem2::Label;
use ims_mem2::Library;

use crate::engine::{pfx_form, NE_NAMES};

// ────────────────────────── 对称剥冠（两端同一函数） ──────────────────────────

/// 冠前缀集合，**长优先**列出（深圳区号 755；跨地市部署时改这里或外置为配置）。
/// 注意 "0" 在最后、"86755" 在 "86" 前、"0755" 在 "0" 前 —— 短冠是长冠的前缀，顺序保证最长匹配。
pub const CROWNS: [&str; 7] = ["0086755", "86755", "0755", "755", "0086", "86", "0"];

/// 过程态：`s` 是某冠的前缀（含恰为冠）—— 用户"正在打冠"，原样查询。
pub fn is_procedural(s: &str) -> bool {
    s.is_empty() || CROWNS.iter().any(|p| s.len() <= p.len() && p.starts_with(s))
}

/// 剥冠：过程态或有冠可剥则剥最长冠，否则原样（本地号码剥不动）。
/// 索引端（生成核心别名）与查询端（归一输入）**同一函数**。
pub fn strip_crown(s: &str) -> &str {
    if is_procedural(s) {
        return s;
    }
    for p in CROWNS {
        if s.len() > p.len() && s.starts_with(p) {
            return &s[p.len()..];
        }
    }
    s
}

// ────────────────────────── 字冠池（原字冠 + 核心别名） ──────────────────────────

const R_CALLED: u8 = 1;
const R_CALLER: u8 = 2;
const R_XFORM: u8 = 4;
/// 中继缺省主叫号码（PRATG.CDEF / N7TG.DI / SIPTG.DCLI）
const R_DN: u8 = 8;

/// 一个原字冠的聚合数据（目录条目展示口径 = 原文，含 roles/nes/hits/落地）。
pub struct OrigEntry {
    pub text: String,
    /// bit0 called / bit1 caller / bit2 transform
    pub roles: u8,
    /// bit i = NE i+1 出现
    pub nes: u8,
    pub hits: [u16; 4],
    pub lands: bool,
}

/// 匹配条目：`key` ∈ {core(orig), orig}，`orig` 指向原字冠下标。
pub struct PoolEntry {
    key: String,
    orig: u32,
}

pub struct PrefixPool {
    origs: Vec<OrigEntry>,
    /// 按 `(key, 原文)` 排序 —— 同核心的原字冠相邻（展示分组）
    entries: Vec<PoolEntry>,
    by_text: HashMap<String, u32>,
}

pub struct HitItem {
    /// 原字冠（目录展示口径，用户拍板）
    pub key: String,
    /// 命中时的匹配键（核心或原文）
    pub mkey: String,
    pub roles: u8,
    pub nes: u8,
    pub hits: [u16; 4],
    pub lands: bool,
}

pub fn roles_strs(roles: u8) -> Vec<&'static str> {
    let mut v = Vec::new();
    if roles & R_CALLED != 0 {
        v.push("called");
    }
    if roles & R_CALLER != 0 {
        v.push("caller");
    }
    if roles & R_XFORM != 0 {
        v.push("transform");
    }
    if roles & R_DN != 0 {
        v.push("dn");
    }
    v
}

impl PrefixPool {
    pub fn len(&self) -> usize {
        self.origs.len()
    }

    pub fn entries_len(&self) -> usize {
        self.entries.len()
    }

    pub fn find(&self, text: &str) -> Option<&OrigEntry> {
        self.by_text.get(text).map(|&i| &self.origs[i as usize])
    }

    /// 查询：`raw` 经剥冠 → 核心前缀匹配条目；返回 (查询键, 总条目, 页, 下一游标)。
    pub fn query(
        &self,
        raw: &str,
        limit: usize,
        after: Option<(&str, &str)>,
    ) -> (String, usize, Vec<HitItem>, Option<(String, String)>) {
        let q = strip_crown(raw).to_string();
        if q.is_empty() {
            return (q, 0, Vec::new(), None);
        }
        let start = self.entries.partition_point(|e| e.key.as_str() < q.as_str());
        let end = start + self.entries[start..]
            .partition_point(|e| e.key.starts_with(q.as_str()));
        let total = end - start;

        let mut i = start;
        if let Some((ak, at)) = after {
            i = self.entries.partition_point(|e| e.key.as_str() < ak);
            if i < end && self.entries[i].key == ak {
                while i < end
                    && self.entries[i].key == ak
                    && self.origs[self.entries[i].orig as usize].text.as_str() <= at
                {
                    i += 1;
                }
            }
            if i < start {
                i = start;
            }
        }

        let mut items = Vec::new();
        while i < end && items.len() < limit {
            let e = &self.entries[i];
            let o = &self.origs[e.orig as usize];
            items.push(HitItem {
                key: o.text.clone(),
                mkey: e.key.clone(),
                roles: o.roles,
                nes: o.nes,
                hits: o.hits,
                lands: o.lands,
            });
            i += 1;
        }
        // keyset 游标 = **本页最后一条**（下一页从严格大于它的地方开始；
        // 若指向"下一条未发送"，那一页的开头会被 <= 判定永久跳过）
        let next = (i > start && i < end)
            .then(|| {
                let e = &self.entries[i - 1];
                (e.key.clone(), self.origs[e.orig as usize].text.clone())
            });
        (q, total, items, next)
    }
}

// ────────────────────────── 用户号码池（排列数组） ──────────────────────────

/// `id = (NE 序 << 24) | 行号`（行号 < 2^24；单网元 ASBR ~37 万行）
pub fn rid_of(ne_pos: usize, row: u32) -> u32 {
    ((ne_pos as u32) << 24) | (row & 0x00ff_ffff)
}

pub struct SubscriberPool {
    /// 按 nsn 升序（前缀查询）
    pub perm: Vec<u32>,
    /// 按逆序 nsn 升序（尾号查询）
    pub rperm: Vec<u32>,
    /// 按**剥冠后本地核心**升序（字冠查询输入≥6 位时的订阅号联合命中）
    pub local: Vec<u32>,
    /// 每个 NE 序在 lib.nes 的真实下标（含无名 NE 偏移）
    ne_slots: Vec<usize>,
}

pub struct SubItem {
    pub nsn: String,
    pub nes: u8,
    /// 同号码的行数
    pub hits: u32,
}

impl SubscriberPool {
    pub fn rows(&self) -> usize {
        self.perm.len()
    }

    /// 派生 nsn（与旧扫描路径同口径：URI 域还原 + nsn_of）
    fn nsn_at(&self, lib: &Library, order: &[u32], i: usize, tail: bool) -> String {
        let id = order[i] as usize;
        let ne_pos = id >> 24;
        let row = (id & 0x00ff_ffff) as u32;
        let ne = &lib.nes[self.ne_slots[ne_pos]];
        let t = ne.table_by_name("ASBR").expect("ASBR 建池时存在");
        let (names, _) = t.col_meta();
        let c = names
            .iter()
            .position(|n| n.eq_ignore_ascii_case("PUI"))
            .expect("ASBR 有 PUI") as u16;
        let sym = t.col_sym(row, c).expect("建池时行有 PUI");
        let raw = lib.interner.decode(sym).to_string();
        let dom = lib.uri_default(self.ne_slots[ne_pos], "ASBR");
        let nsn = crate::engine::nsn_of(&lib.rehydrate_uri(raw, dom));
        if tail {
            nsn.chars().rev().collect()
        } else {
            nsn
        }
    }

    /// 前缀 / 尾号查询。`total` 为行口径（去重计数要全扫，太贵）；items 已按号码去重合并。
    #[allow(clippy::too_many_arguments)]
    pub fn query(
        &self,
        lib: &Library,
        kind: &str,
        arg: &str,
        limit: usize,
        after: Option<&str>,
    ) -> (usize, Vec<SubItem>, Option<String>) {
        let tail = kind == "tail";
        let order = if tail { &self.rperm } else { &self.perm };
        if arg.is_empty() {
            return (0, Vec::new(), None);
        }
        let probe: String = if tail {
            arg.chars().rev().collect()
        } else {
            arg.to_string()
        };
        // 二分：闭包里按需派生（每次比较一次派生，log2(67万)≈20 次）
        let start = {
            let mut lo = 0usize;
            let mut hi = order.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if self.nsn_at(lib, order, mid, tail) < probe {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let end = {
            let mut lo = start;
            let mut hi = order.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if self.nsn_at(lib, order, mid, tail).starts_with(&probe) {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let total = end - start;

        let mut i = start;
        if let Some(a) = after {
            let pa: String = if tail { a.chars().rev().collect() } else { a.to_string() };
            while i < end && self.nsn_at(lib, order, i, tail) <= pa {
                i += 1;
            }
        }

        // 按号码去重合并（同号码行在排好序的数组里相邻）
        let mut items: Vec<SubItem> = Vec::new();
        let mut last_sent: Option<String> = None;
        while i < end && items.len() < limit {
            let k = self.nsn_at(lib, order, i, tail);
            let mut nes = 0u8;
            let mut hits = 0u32;
            while i < end && self.nsn_at(lib, order, i, tail) == k {
                nes |= 1u8 << (order[i] as usize >> 24);
                hits += 1;
                i += 1;
            }
            let nsn = if tail { k.chars().rev().collect() } else { k };
            last_sent = Some(nsn.clone());
            items.push(SubItem { nsn, nes, hits });
        }
        // keyset 游标 = 本页最后一个号码（用户池单键）
        let next = if i < end && !items.is_empty() { last_sent } else { None };
        (total, items, next)
    }

    /// 订阅号**剥冠核心**前缀查询：用户输本地号（如 `82001`）也能命中本地冠形态
    /// （`75582001000…`）。匹配键 = `strip_crown(nsn)`（`local` 数组已按它排序），
    /// 输入同样剥冠；纯冠输入（剥后为空，如 `755`）退化为原样匹配（通常 0）。
    pub fn query_core(
        &self,
        lib: &Library,
        arg: &str,
        limit: usize,
        after: Option<&str>,
    ) -> (usize, Vec<SubItem>, Option<String>) {
        let order = &self.local;
        if arg.is_empty() || limit == 0 {
            return (0, Vec::new(), None);
        }
        let mut probe = strip_crown(arg).to_string();
        if probe.is_empty() {
            probe = arg.to_string();
        }
        let key_of = |k: &str| strip_crown(k).to_string();
        // 二分：local 已按剥冠核心升序
        let start = {
            let mut lo = 0usize;
            let mut hi = order.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if key_of(&self.nsn_at(lib, order, mid, false)).as_str() < probe.as_str() {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let end = {
            let mut lo = start;
            let mut hi = order.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if key_of(&self.nsn_at(lib, order, mid, false))
                    .as_str()
                    .starts_with(&probe)
                {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let total = end - start;

        let mut i = start;
        if let Some(a) = after {
            // 双键回退：先按剥冠核心（< pa），再在同 core 组内按 nsn（<= a 的全跳过），
            // 避免把上一页最后一条的同 core 组整组重新吐出（分页边界 bug）。
            let pa = strip_crown(a).to_string();
            while i < end {
                let nsn = self.nsn_at(lib, order, i, false);
                let core = strip_crown(&nsn);
                let before = core < pa.as_str()
                    || (core == pa.as_str() && nsn.as_str() <= a);
                if !before {
                    break;
                }
                i += 1;
            }
        }

        // 按 nsn 原文去重合并（同号码行在剥冠排序里相邻）
        let mut items: Vec<SubItem> = Vec::new();
        let mut last_sent: Option<String> = None;
        while i < end && items.len() < limit {
            let k = self.nsn_at(lib, order, i, false);
            let mut nes = 0u8;
            let mut hits = 0u32;
            while i < end && self.nsn_at(lib, order, i, false) == k {
                nes |= 1u8 << (order[i] as usize >> 24);
                hits += 1;
                i += 1;
            }
            last_sent = Some(k.clone());
            items.push(SubItem { nsn: k, nes, hits });
        }
        let next = if i < end && !items.is_empty() { last_sent } else { None };
        (total, items, next)
    }

    /// 订阅号本地核心（剥冠后）前缀索引：输入核心≥6 位时把匹配的订阅号追加到字冠结果
    /// 末尾（角色=户）。按号码去重合并，返回条数 ≤ limit。
    pub fn local_prefix(&self, lib: &Library, core: &str, limit: usize) -> Vec<SubItem> {
        if core.is_empty() || limit == 0 {
            return Vec::new();
        }
        let order = &self.local;
        // 排序键 = 剥冠后的本地核心（与查询剥冠同一函数）
        let key_of = |k: &str| strip_crown(k).to_string();
        let start = {
            let mut lo = 0usize;
            let mut hi = order.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if key_of(&self.nsn_at(lib, order, mid, false)).as_str() < core {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let end = {
            let mut lo = start;
            let mut hi = order.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if key_of(&self.nsn_at(lib, order, mid, false)).as_str().starts_with(core) {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let mut items: Vec<SubItem> = Vec::new();
        let mut i = start;
        while i < end && items.len() < limit {
            let k = self.nsn_at(lib, order, i, false);
            let mut nes = 0u8;
            let mut hits = 0u32;
            while i < end && self.nsn_at(lib, order, i, false) == k {
                nes |= 1u8 << (order[i] as usize >> 24);
                hits += 1;
                i += 1;
            }
            items.push(SubItem { nsn: k, nes, hits });
        }
        items
    }
}

// ────────────────────────── 落地链 & DNC 规则（构建期） ──────────────────────────

/// DNC 变换规则（呼入主叫补齐展示用）：dcx → (dct, nd, dcl)
pub struct DncRule {
    pub dct: String,
    pub nd: String,
    pub dcl: u8,
}

/// 每网元的「rsc → 是否落地」集合：`rsc → r(rtana) → sr(rt 全宽) → src(srt) → o → ofc.dol=LOW`
fn lands_rsc_set(lib: &Library, ne: &ims_mem2::NeStore) -> HashSet<u16> {
    let mut out = HashSet::new();
    // 落地局向（DOL=LOW 的 OFC.O）
    let mut low_ofc: HashSet<u16> = HashSet::new();
    if let Some(t) = ne.table_by_name("OFC") {
        let (names, _) = t.col_meta();
        let col = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|i| i as u16);
        if let (Some(oc), Some(dc)) = (col("O"), col("DOL")) {
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                if let Some(d) = t.col_sym(row, dc) {
                    if lib.interner.decode(d) == "LOW" {
                        if let Some(o) = t.col_u32(row, oc) {
                            low_ofc.insert(o as u16);
                        }
                    }
                }
            }
        }
    }
    // 子路由（SRT.SRC）→ 局向（O）在落地集合 → 该子路由落地
    let mut landing_src: HashSet<u16> = HashSet::new();
    if let Some(t) = ne.table_by_name("SRT") {
        let (names, _) = t.col_meta();
        let col = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|i| i as u16);
        if let (Some(sc), Some(oc)) = (col("SRC"), col("O")) {
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                if let (Some(src), Some(o)) = (t.col_u32(row, sc), t.col_u32(row, oc)) {
                    if low_ofc.contains(&(o as u16)) {
                        landing_src.insert(src as u16);
                    }
                }
            }
        }
    }
    // 路由（RT）：子路由组 SR 全宽 28 位，任一落地的子路由 → 该路由落地
    let mut landing_r: HashSet<u16> = HashSet::new();
    if let Some(t) = ne.table_by_name("RT") {
        let (names, _) = t.col_meta();
        let col = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|i| i as u16);
        if let (Some(rc), Some(sc)) = (col("R"), col("SR")) {
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                let Some(r) = t.col_u32(row, rc) else { continue };
                if let Some(g) = t.group_of(row, sc) {
                    for i in 0..g.vals.len() {
                        if let Some(sr) = g.get(i) {
                            if landing_src.contains(&sr) {
                                landing_r.insert(r as u16);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    // 路由分析（RTANA）：rsc → r；r 落地 → rsc 落地
    if let Some(t) = ne.table_by_name("RTANA") {
        let (names, _) = t.col_meta();
        let col = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|i| i as u16);
        if let (Some(rc), Some(c)) = (col("RSC"), col("R")) {
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                if let (Some(rsc), Some(r)) = (t.col_u32(row, rc), t.col_u32(row, c)) {
                    if landing_r.contains(&(r as u16)) {
                        out.insert(rsc as u16);
                    }
                }
            }
        }
    }
    let _ = lib;
    out
}

fn dnc_rules_dec(lib: &Library, ne: &ims_mem2::NeStore) -> HashMap<u16, DncRule> {
    let mut out = HashMap::new();
    let Some(t) = ne.table_by_name("DNC") else { return out };
    let (names, _) = t.col_meta();
    let col = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|i| i as u16);
    let (xc, tc) = (col("DCX"), col("DCT"));
    let (Some(xc), Some(tc)) = (xc, tc) else { return out };
    let nc = col("ND");
    let lc = col("DCL");
    // DCT 是枚举列（kind 'e'）：值=标签池下标，需 `label_sym` 解回词（find 只读，勿 register）
    let dct_pool = lib.pools.find("DNC.DCT");
    for row in 0..t.len() as u32 {
        if !t.is_alive(row) {
            continue;
        }
        let Some(dcx) = t.col_u32(row, xc) else { continue };
        let dct = match dct_pool {
            Some(pool) => t
                .col_u32(row, tc)
                .and_then(|v| u8::try_from(v).ok())
                .and_then(Label::from_u8_0)
                .and_then(|l| lib.pools.label_sym(pool, l))
                .map(|s| lib.interner.decode(s).to_string())
                .unwrap_or_default(),
            None => String::new(),
        };
        let nd = match nc {
            Some(c) => t
                .col_sym(row, c)
                .map(|s| lib.interner.decode(s).to_string())
                .unwrap_or_default(),
            None => String::new(),
        };
        let dcl = match lc {
            Some(c) => t.col_u32(row, c).unwrap_or(0) as u8,
            None => 0,
        };
        out.insert(dcx as u16, DncRule { dct, nd, dcl });
    }
    out
}

// ────────────────────────── 构建入口 ──────────────────────────

/// 取行/列的字冠文本。真实库里 PFX/CLI 均为 `PfxText`（kind 't'：原文驻留 interner，
/// 形如 `K'0086755`/`0086755`），走 `col_sym`+interner；`pfx_text`（A3 的 PfxNum
/// 伴生列口径）仅作兼容兜底。实证：examples/pfx_lpm_dump、scan_cost 同此路径。
fn pfx_val(lib: &Library, t: &dyn TableApi, row: u32, col: u16) -> Option<String> {
    t.pfx_text(row, col)
        .or_else(|| t.col_sym(row, col).map(|s| lib.interner.decode(s).to_string()))
}


/// 目录池构建结果：字冠池 / 用户池 / 落地集（按 NE 序）/ DNC 规则（按 NE 序）。
pub type PoolBundle = (
    PrefixPool,
    SubscriberPool,
    Vec<HashSet<u16>>,
    Vec<HashMap<u16, DncRule>>,
);

/// 一次构建全部目录池。
pub fn build_all(lib: &Library) -> PoolBundle {
    let t0 = std::time::Instant::now();

    // NE 定位：NE_NAMES 序 → lib.nes 真实下标（lib.nes[0] 可能是无名 NE）
    let ne_slots: Vec<usize> = NE_NAMES
        .iter()
        .map(|n| lib.ne_by_name(n).map(|ne| lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap_or(usize::MAX)).unwrap_or(usize::MAX))
        .collect();

    // 落地集 & DNC 规则（每网元一份）
    let lands: Vec<HashSet<u16>> = ne_slots
        .iter()
        .map(|&s| if s == usize::MAX { HashSet::new() } else { lands_rsc_set(lib, &lib.nes[s]) })
        .collect();
    let dnc: Vec<HashMap<u16, DncRule>> = ne_slots
        .iter()
        .map(|&s| if s == usize::MAX { HashMap::new() } else { dnc_rules_dec(lib, &lib.nes[s]) })
        .collect();

    // ── 字冠池聚合 ──
    // agg: 原文 → (roles, nes, hits[4], lands)；hits=[called/caller/xform/中继缺省主叫]
    let mut agg: HashMap<String, (u8, u8, [u16; 4], bool)> = HashMap::new();
    for (pos, &slot) in ne_slots.iter().enumerate() {
        if slot == usize::MAX {
            continue;
        }
        let ne = &lib.nes[slot];
        let ne_bit = 1u8 << pos;
        // CNACLD（被叫）；SPCHG 标记的行 = 号码变化承载 → 复用 xform 角色
        if let Some(t) = ne.table_by_name("CNACLD") {
            let (names, _) = t.col_meta();
            let col = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|i| i as u16);
            let (pc, rc, sc) = (col("PFX"), col("RSC"), col("SPCHG"));
            if let Some(pc) = pc {
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) {
                        continue;
                    }
                    let Some(v) = pfx_val(lib, t, row, pc) else { continue };
                    let k = pfx_form(v);
                    let e = agg.entry(k).or_insert((0, 0, [0; 4], false));
                    e.0 |= R_CALLED;
                    e.1 |= ne_bit;
                    e.2[0] += 1;
                    if let Some(sc) = sc {
                        if t.col_sym(row, sc).is_some() {
                            e.0 |= R_XFORM;
                            e.2[2] += 1;
                        }
                    }
                    // 落地标志（决议 4）：该行的 rsc 是否在落地集
                    if let Some(rsc_col) = rc {
                        if let Some(rsc_v) = t.col_u32(row, rsc_col) {
                            if lands[pos].contains(&(rsc_v as u16)) {
                                e.3 = true;
                            }
                        }
                    }
                }
            }
        }
        // CLRDSN（主叫甄别）→ 本身 + RDCX→DNC 变换后目标字冠
        if let Some(t) = ne.table_by_name("CLRDSN") {
            let (names, _) = t.col_meta();
            let col = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|i| i as u16);
            let (cc, rxc) = (col("CLI"), col("RDCX"));
            if let Some(cc) = cc {
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) {
                        continue;
                    }
                    let Some(v) = pfx_val(lib, t, row, cc) else { continue };
                    let k = pfx_form(v);
                    let e = agg.entry(k.clone()).or_insert((0, 0, [0; 4], false));
                    e.0 |= R_CALLER;
                    e.1 |= ne_bit;
                    e.2[1] += 1;
                    // rdcx=INS nd → 变换目标字冠 = INS 前缀 + 短字冠（如 851+2=8512）
                    if let Some(rxc) = rxc {
                        if let Some(r) = t.col_u32(row, rxc) {
                            if let Some(rule) = dnc[pos].get(&(r as u16)) {
                                if rule.dct == "INS" && !rule.nd.is_empty() {
                                    let pre = pfx_form(rule.nd.clone());
                                    let tk = format!("{pre}{k}");
                                    if tk != k {
                                        let e2 = agg.entry(tk).or_insert((0, 0, [0; 4], false));
                                        e2.0 |= R_XFORM;
                                        e2.1 |= ne_bit;
                                        e2.2[2] += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // 中继缺省主叫号码（PRATG.CDEF / N7TG.DI / SIPTG.DCLI）也进目录
        for fam in FAMILIES {
            let Some(t) = ne.table_by_name(fam.moc) else { continue };
            let (names, _) = t.col_meta();
            let col = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|i| i as u16);
            let Some(dc) = col(fam.dn_param) else { continue };
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                let Some(v) = pfx_val(lib, t, row, dc) else { continue };
                let k = pfx_form(v);
                if k.is_empty() {
                    continue;
                }
                let e = agg.entry(k).or_insert((0, 0, [0; 4], false));
                e.0 |= R_DN;
                e.1 |= ne_bit;
                e.2[3] += 1;
            }
        }
    }

    let mut origs: Vec<OrigEntry> = Vec::with_capacity(agg.len());
    let mut entries: Vec<PoolEntry> = Vec::with_capacity(agg.len() * 2);
    for (text, (roles, nes, hits, lands_flag)) in agg {
        origs.push(OrigEntry { text, roles, nes, hits, lands: lands_flag });
    }
    origs.sort_by(|a, b| a.text.cmp(&b.text));
    let by_text: HashMap<String, u32> = origs
        .iter()
        .enumerate()
        .map(|(i, o)| (o.text.clone(), i as u32))
        .collect();
    for (i, o) in origs.iter().enumerate() {
        let core = strip_crown(&o.text);
        entries.push(PoolEntry { key: core.to_string(), orig: i as u32 });
        if core != o.text {
            entries.push(PoolEntry { key: o.text.clone(), orig: i as u32 });
        }
    }
    entries.sort_by(|a, b| {
        a.key.cmp(&b.key)
            .then_with(|| origs[a.orig as usize].text.cmp(&origs[b.orig as usize].text))
    });
    let prefix = PrefixPool { origs, entries, by_text };

    // ── 用户号码池（排列数组，不复制键串；细节见 build_subs_perm） ──
    let t_sub = std::time::Instant::now();
    let perm = build_subs_perm(lib, &ne_slots, false);
    let rperm = build_subs_perm(lib, &ne_slots, true);
    let local = build_subs_local(lib, &ne_slots);
    let subs = SubscriberPool { perm, rperm, local, ne_slots };

    eprintln!(
        "池构建 {:.2}s（用户池 {:.2}s）：字冠原文 {} · 匹配条目 {} · 用户行 {}（perm+{} rperm+{} local+{}）",
        t0.elapsed().as_secs_f64(),
        t_sub.elapsed().as_secs_f64(),
        prefix.len(),
        prefix.entries_len(),
        subs.rows(),
        subs.perm.len(),
        subs.rperm.len(),
        subs.local.len()
    );

    (prefix, subs, lands, dnc)
}

/// 用户池排列数组构建（按派生 nsn 或其逆串排序）。
fn build_subs_perm(lib: &Library, ne_slots: &[usize], tail: bool) -> Vec<u32> {
    struct TmpKey {
        k: String,
        id: u32,
    }
    let mut v: Vec<TmpKey> = Vec::new();
    for (pos, &slot) in ne_slots.iter().enumerate() {
        if slot == usize::MAX {
            continue;
        }
        let ne = &lib.nes[slot];
        let Some(t) = ne.table_by_name("ASBR") else { continue };
        let (names, _) = t.col_meta();
        let Some(c) = names.iter().position(|x| x.eq_ignore_ascii_case("PUI")) else {
            continue;
        };
        let c = c as u16;
        let dom = lib.uri_default(slot, "ASBR");
        for row in 0..t.len() as u32 {
            if !t.is_alive(row) {
                continue;
            }
            if let Some(sym) = t.col_sym(row, c) {
                let raw = lib.interner.decode(sym).to_string();
                let nsn = crate::engine::nsn_of(&lib.rehydrate_uri(raw, dom));
                let k = if tail { nsn.chars().rev().collect() } else { nsn };
                v.push(TmpKey { k, id: rid_of(pos, row) });
            }
        }
    }
    v.sort_by(|a, b| a.k.cmp(&b.k).then_with(|| a.id.cmp(&b.id)));
    v.into_iter().map(|x| x.id).collect()
}

/// 用户池本地核心排列（按剥冠后的本地号码升序）——ASBR 联合（输入≥6 位）用。
fn build_subs_local(lib: &Library, ne_slots: &[usize]) -> Vec<u32> {
    struct TmpKey {
        k: String,
        id: u32,
    }
    let mut v: Vec<TmpKey> = Vec::new();
    for (pos, &slot) in ne_slots.iter().enumerate() {
        if slot == usize::MAX {
            continue;
        }
        let ne = &lib.nes[slot];
        let Some(t) = ne.table_by_name("ASBR") else { continue };
        let (names, _) = t.col_meta();
        let Some(c) = names.iter().position(|x| x.eq_ignore_ascii_case("PUI")) else {
            continue;
        };
        let c = c as u16;
        let dom = lib.uri_default(slot, "ASBR");
        for row in 0..t.len() as u32 {
            if !t.is_alive(row) {
                continue;
            }
            if let Some(sym) = t.col_sym(row, c) {
                let raw = lib.interner.decode(sym).to_string();
                let nsn = crate::engine::nsn_of(&lib.rehydrate_uri(raw, dom));
                let k = strip_crown(&nsn).to_string();
                v.push(TmpKey { k, id: rid_of(pos, row) });
            }
        }
    }
    v.sort_by(|a, b| a.k.cmp(&b.k).then_with(|| a.id.cmp(&b.id)));
    v.into_iter().map(|x| x.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedural_and_strip() {
        // 过程态（正在打冠）
        for s in ["", "0", "00", "008", "0086", "00867", "008675", "0086755",
                  "8", "86", "867", "8675", "86755",
                  "7", "75", "755", "0", "07", "075", "0755"] {
            assert_eq!(strip_crown(s), s, "过程态 {s:?} 应原样");
        }
        // 剥最长冠
        assert_eq!(strip_crown("0086755235165"), "235165", "剥 0086755");
        assert_eq!(strip_crown("0755235165"), "235165", "剥 0755");
        assert_eq!(strip_crown("75582001577"), "82001577", "剥 755");
        assert_eq!(strip_crown("86260013"), "260013", "剥 86");
        assert_eq!(strip_crown("86755123"), "123", "86755 匹配时优先于 86");
        assert_eq!(strip_crown("86757446432"), "757446432", "86755 第5位不匹配 → 落到 86");
        assert_eq!(strip_crown("0086757446432"), "757446432", "剥 0086（非 0086755 前缀）");
        assert_eq!(strip_crown("757446432"), "757446432", "本地号剥不动");
        assert_eq!(strip_crown("212"), "212", "裸固话原样");
    }

    #[test]
    fn strip_matches_sim_claims() {
        // temp/q_core_sim.py 的关键键位（设计对拍的口径锁定）：
        // 输入 7552 / 00867552 / 07552 都应归一到核心 "2"
        assert_eq!(strip_crown("7552"), "2");
        assert_eq!(strip_crown("00867552"), "2");
        assert_eq!(strip_crown("07552"), "2");
        // 075582001 → 82001（同一核心下命中 008675582001577 与裸 75582001577 两形态）
        assert_eq!(strip_crown("075582001"), "82001");
        assert_eq!(strip_crown("008675582001577"), "82001577");
        assert_eq!(strip_crown("75582001577"), "82001577");
        // 86* 双义（86 = 国家码冠；86… 也可作 0086755|8 字头的核心开头）
        assert_eq!(strip_crown("86551234567"), "551234567");
    }

    #[test]
    fn crown_order_longest_first() {
        // 关键性质：若长冠以短冠为前缀，长冠必须排在前面（否则短冠抢先匹配）。
        // （长度不严格降序是允许的：755 与 0086 互不为前缀，先后无关）
        for (i, a) in CROWNS.iter().enumerate() {
            for (j, b) in CROWNS.iter().enumerate() {
                if i < j {
                    assert!(!b.starts_with(a), "冠 {a} 在 {b} 前但 {b} 以 {a} 开头，会抢先匹配");
                }
            }
        }
        // 长冠优先的实际效果（不是长度序，是"互斥短冠不打架"）
        assert_eq!(strip_crown("86755123"), "123", "86755 优先于 86");
        assert_eq!(strip_crown("0086755123"), "123", "0086755 优先于 0086");
        assert_eq!(strip_crown("0755123"), "123", "0755 优先于 0");
    }

    #[test]
    fn prefix_pool_query_and_display() {
        // 手工小池：验证 (核心,原文) 双条目、排序、游标、展示展开
        let origs = vec![
            OrigEntry { text: "0086755235165".into(), roles: R_CALLED, nes: 0b11, hits: [3, 0, 0, 0], lands: true },
            OrigEntry { text: "235165".into(), roles: R_CALLED, nes: 0b01, hits: [1, 0, 0, 0], lands: false },
            OrigEntry { text: "0755235".into(), roles: R_XFORM, nes: 0b10, hits: [0, 0, 2, 0], lands: false },
        ];
        let mut entries = Vec::new();
        for (i, o) in origs.iter().enumerate() {
            let core = strip_crown(&o.text).to_string();
            entries.push(PoolEntry { key: core, orig: i as u32 });
            if strip_crown(&o.text) != o.text {
                entries.push(PoolEntry { key: o.text.clone(), orig: i as u32 });
            }
        }
        entries.sort_by(|a, b| {
            a.key.cmp(&b.key).then_with(|| origs[a.orig as usize].text.cmp(&origs[b.orig as usize].text))
        });
        let by_text = origs.iter().enumerate().map(|(i, o)| (o.text.clone(), i as u32)).collect();
        let pool = PrefixPool { origs, entries, by_text };

        // 输入 7552 → 核心前缀 "2"：命中全部核心 2 开头的条目，按 (核心, 原文) 排序
        // （"0755235" 的核心是 "235"，排在核心 "235165" 的两条前面）
        let (q, total, items, _) = pool.query("7552", 50, None);
        assert_eq!(q, "2");
        assert_eq!(total, 3);
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, vec!["0755235", "0086755235165", "235165"], "同核心条目按原文相邻");
        // 每条都是原字冠（展示口径），roles/lands 随原文
        assert!(items[1].lands);
        assert!(!items[2].lands);

        // 输入 0086755（恰为冠，过程态）→ 命中 0086 开头的原文条目
        let (q2, total2, items2, _) = pool.query("0086755", 50, None);
        assert_eq!(q2, "0086755");
        assert_eq!(total2, 1);
        assert_eq!(items2[0].key, "0086755235165");

        // 输入 7（过程态）→ 原文 7 开头（本池只有 0086755… 原文键，7 开头原文无）+ 核心 7 开头（无）
        let (q3, total3, _, _) = pool.query("7", 50, None);
        assert_eq!(q3, "7");
        assert_eq!(total3, 0);

        // 分页游标：(match,key) 键集 —— 游标 = 本页最后一条；下一页从严格大于处开始
        let (_, _, page1, next) = pool.query("7552", 2, None);
        assert_eq!(page1.len(), 2);
        let (nk, nt) = next.unwrap();
        assert_eq!((nk.as_str(), nt.as_str()), ("235165", "0086755235165"));
        let (_, _, page2, next2) = pool.query("7552", 2, Some((&nk, &nt)));
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].key, "235165");
        assert!(next2.is_none());
    }

    #[test]
    fn rid_roundtrip() {
        let id = rid_of(5, 123456);
        assert_eq!(id >> 24, 5);
        assert_eq!(id & 0x00ff_ffff, 123456);
    }

    #[test]
    fn dn_role_and_roles_strs() {
        // 中继缺省主叫(R_DN) 与其它角色组合的角色字符串输出
        assert_eq!(roles_strs(R_DN), vec!["dn"]);
        assert_eq!(roles_strs(R_CALLED | R_DN), vec!["called", "dn"]);
        assert_eq!(roles_strs(R_CALLER | R_XFORM | R_DN), vec!["caller", "transform", "dn"]);
        // 位掩码不冲突（四类角色互斥占用低 4 位）
        assert_eq!(
            R_CALLED | R_CALLER | R_XFORM | R_DN,
            0b1111,
            "四角色位不重叠"
        );
    }
}

