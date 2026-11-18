//! PRA 家族（P0.4e → **现按 D54 落地**）。
//!
//! 现状：**三张表**，但 `PRATKC` 已从"每电路一行"改成 **"块即行"**（一行 = 一个 PCM 块），
//! 行数 140,032 → 4,376（steady RSS 135.0 → 123.0 MB）。`PRALNK` 按 **D55** 保持独立
//! （并入收益 0.1 MB，低于 1 MB 阈值，且要额外处理 2 行孤儿）。
//!
//! 口径（与 ims_mem 的 `pra.rs` 一致的部分）：
//! - `PRATG` 键 `TG`，`LINK` 引用 `PRALNK.PLN`（实测 1:1，4,376/4,376 全命中）；
//! - `PRALNK` 键 `PLN`（实测块序反转：PRALNK 先到、PRATG 后到）；
//! - `PRATKC` 键 `(MN,TG,SC0)`，一行代表一个 32 时隙的 PCM 块。
//!
//! # PRATKC 的"块即行"（D54，取代 D30 的"不折叠"）
//!
//! 实测（6 文件，`tools/probe_fold.py`）：
//! - 4,376 个 `(MN,TG)` 组，每组**恰好 32 条** `SC`，`SC` 连续、`EC == SC`、
//!   `TID - SC` 恒定、`CS ∈ {USE,UNU}`、`TGN` 与 `TIDPFX` 组内恒定、**0 例外**；
//! - 也就是说：140,032 条电路 = 4,376 个 PCM 块，块内一切都能由
//!   `(sc0, ec0, tid0, use_mask)` 重算出来。
//!
//! 所以 `PRATKC` 的一行 = **一个 PCM 块**（不是一条电路）：
//!
//! | 列 | 含义 | 展开回扁平行 |
//! | --- | --- | --- |
//! | `MN/TG/TGN` | 组标识（组内恒定） | 原样 |
//! | `SC0/EC0/TID0` | 槽 0 的 `SC/EC/TID` | `SC0+i` / `EC0+i` / `TID0+i` |
//! | `SLOTS` | 本行代表几条电路（32 = 满块；1 = 块化失败的例外） | `i ∈ 0..SLOTS` |
//! | `USE_LO/USE_HI` | 槽 `i` 是 USE 还是 UNU（32 位位图拆两半） | `bit i` |
//! | `TIDPFX` | 组内恒定的枚举 | 原样 |
//!
//! **`SLOTS` 是"不丢信息"的关键**（D52）：形态不符的组不静默丢弃，而是整组回退成
//! `SLOTS=1` 的逐条行（`SC0/EC0/TID0` 就是那一条自己的值），展开后与原文逐字段相同。
//! 例外数由 `exceptions()` 显式计数，验收门断言它为 0。
//!
//! 为什么用"块即行"而不是 D53 原计划的"行内 `(block_start, block_len)` + 旁挂 arena"：
//! 后者会让 `PRATKC` 从表册里消失，于是 SCD2/投影/Ref/导出四条通路都要特判；
//! 而块即行只把行数除以 32 —— 行号稳定、版本链、快照删除、投影、Ref 全部照旧可用
//! （代价：列集从 `SC/EC/TID/CS` 变成"块 + 位图"，`cur_PRATKC` 变成块形态，见 README）。
//!
//! 未做的两件（`PRALNK` 并入 `PRATG`、`N7TKC` 稀疏化）在 README 里记了原因：
//! 实测净收益只有 ≈0.1 MB / ≈0.2 MB，而两者都要动 Ref 列号与外部口径。

use crate::label::{Label, LabelCols};
use crate::mml_table;
use crate::scan::{Op, Params};
use crate::sym::{Interner, Sym};
use crate::vhist::ColStore;

mml_table! {
    PRATG, Pratg,
    key [tg],
    tg: U32,
    tgn: Text,
    mgw: Eid,
    src: U32,
    link: U32,
    cdef: Pfx,
    csc: U16,
    rchs: U32,
    ehc: Bool,
    icr: Text,
    ocr: Text,
    eva: Text,
    svrctrl: Text,
    svrctrls: Text,
    svrctrlt: Text,
    listofcodec: Text,
    ioi: Text,
    isbf: Bool,
    isdp: Bool,
    nicf: Bool,
    lmtgrp: U16,
    maxi: U32,
    mtg: U32,
    tclityp: Enum,
    tgms: Enum,
    // ---- 以下 11 列 = PRALNK 按 LINK = PLN 并入（D53 步骤 1）----
    // 一律**追加在末尾**，前 25 列的列号不动（refs_gen 的 from_col 才不会漂）；
    // 列名与 PRALNK 的参数名一致，`tools/gen_refs.py` 才能把 PRALNK 的边改写到 PRATG。
    lnkname: Text,
    scn: U32,
    mn: U16,
    lks: U16,
    binifid: U32,
    sigt: Enum,
    ogcrl: U32,
    ncf: Enum,
    rcx: U32,
    cci: U32,
    nv: Enum,
}

/// `PRALNK` 的一行（`PLN` 之外的 11 列）。既是批内缓冲形态，也是 dump 的展开形态。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PralnkCols {
    pub lnkname: Option<Sym>,
    pub scn: Option<u32>,
    pub mn: Option<u16>,
    pub lks: Option<u16>,
    pub binifid: Option<u32>,
    pub sigt: Option<Label>,
    pub ogcrl: Option<u32>,
    pub ncf: Option<Label>,
    pub rcx: Option<u32>,
    pub cci: Option<u32>,
    pub nv: Option<Label>,
}

/// `PRATG` 的外壳（D53 步骤 1）：`PRALNK` 的 12 列按 `LINK = PLN` 并进 `PRATG` 同一行。
///
/// 为什么**批末**才写：实测 6/6 文件里 `PRALNK` 块都出现在 `PRATG` 块**之前**，
/// 而 `PRATG` 的 `ADD` 是整行覆盖 —— 立刻写会被随后的 ADD 冲掉（D33 的坑，D48 已经踩过一次）。
/// 所以 `PRALNK` 只在批内登记，批末再 `set_cell` 到 `LINK == PLN` 的那一行上。
///
/// 不丢信息（D52）：
/// - 本批见了 `PRALNK` 但没有任何 `PRATG` 行引用它 → 进 `orphans`（实测 2 条），dump 照旧展开；
/// - 上一批落过、本批没出现的行 → 这 11 列清回 `Default`（快照语义）；
/// - 枚举池满（>255 个词）→ 计 `enum_overflow`，不静默改值。
#[derive(Default)]
pub struct PratgShim {
    pub core: PratgTable,
    /// 本批的 `PRALNK`：`PLN → 11 列`
    pending: std::collections::HashMap<u32, PralnkCols>,
    /// 本批没被任何 `PRATG` 行引用的 `PRALNK`（跨批累计，按 PLN 去重）
    orphans: std::collections::BTreeMap<u32, PralnkCols>,
    /// 上一批落过 `PRALNK` 的 PRATG 行号
    prev: std::collections::HashSet<u32>,
    /// 本批 `PRATG` 真的落过的 TG（缺陷②：只给这些行写列）
    batch_tgs: std::collections::HashSet<u32>,
    /// 成功写进 PRATG 的 `PRALNK` 行数（累计）
    applied: u64,
    /// 枚举池溢出次数（应为 0）
    enum_overflow: u64,
}

impl std::ops::Deref for PratgShim {
    type Target = PratgTable;
    fn deref(&self) -> &PratgTable {
        &self.core
    }
}
impl std::ops::DerefMut for PratgShim {
    fn deref_mut(&mut self) -> &mut PratgTable {
        &mut self.core
    }
}

/// `PRATG` 某列的列号（按列名查一次；声明序变了也不会错位）。
fn col_of(name: &str) -> u16 {
    <PratgHooks as crate::table::TableHooks>::COL_NAMES
        .iter()
        .position(|n| *n == name)
        .unwrap_or_else(|| panic!("PRATG 声明里应有 {name} 列")) as u16
}

#[inline]
fn cv16(v: Option<u16>) -> crate::vhist::ColVal {
    v.map(|x| crate::vhist::ColVal::small(crate::val::Val32::set_masked(x as u32)))
        .unwrap_or_default()
}
#[inline]
fn cv32(v: Option<u32>) -> crate::vhist::ColVal {
    v.map(|x| crate::vhist::ColVal::small(crate::val::Val32::set(x).unwrap_or_default()))
        .unwrap_or_default()
}
#[inline]
fn cvlab(v: Option<Label>) -> crate::vhist::ColVal {
    v.map(|l| crate::vhist::ColVal::small(crate::val::Val32::set_masked(l.as_u8() as u32)))
        .unwrap_or_default()
}

/// 把一个枚举词的原文换成池内标签；池满返回 `(None, 1)`（由调用方计 `enum_overflow`）。
fn enum_of(
    it: &mut Interner,
    pools: &mut LabelCols,
    name: &'static str,
    v: &[u8],
) -> (Option<Label>, u64) {
    let id = pools.register(name);
    let sym = it.intern(crate::frombytes::unquote(v));
    match pools.label_of(id, sym) {
        Some(l) => (Some(l), 0),
        None => (None, 1),
    }
}

impl PratgShim {
    pub fn new(batch: i64) -> Self {
        let mut core = PratgTable::new();
        core.set_batch(batch);
        PratgShim {
            core,
            ..Default::default()
        }
    }

    /// 成功并入 `PRATG` 的 `PRALNK` 行数（累计）。
    pub fn applied(&self) -> u64 {
        self.applied
    }

    /// 本批没被引用的 `PRALNK` 行数（跨批去重后的集合大小）。
    pub fn orphan_count(&self) -> usize {
        self.orphans.len()
    }

    pub fn enum_overflow(&self) -> u64 {
        self.enum_overflow
    }

    /// 收一条 `PRATG`（直接进表；本批的 `PRALNK` 在批末补列）。
    pub fn apply_tg(&mut self, op: Op, body: &[u8], it: &mut Interner, pools: &mut LabelCols) {
        if let Some(v) = crate::scan::first_param(body, b"TG") {
            if let Some(tg) = crate::frombytes::parse_u32(v) {
                self.batch_tgs.insert(tg);
            }
        }
        self.core.apply(op, body, it, pools);
    }

    /// 收一条 `PRALNK`：只登记，批末落盘。
    pub fn apply_lnk(&mut self, op: Op, body: &[u8], it: &mut Interner, pools: &mut LabelCols) {
        let mut pln: Option<u32> = None;
        let mut c = PralnkCols::default();
        let mut overflow = 0u64;
        for (k, v) in Params::new(body) {
            if k.eq_ignore_ascii_case(b"PLN") {
                pln = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"LNKNAME") {
                c.lnkname = Some(it.intern(crate::frombytes::unquote(v)));
            } else if k.eq_ignore_ascii_case(b"SCN") {
                c.scn = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"MN") {
                c.mn = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"LKS") {
                c.lks = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"BINIFID") {
                c.binifid = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"SIGT") {
                let (l, of) = enum_of(it, pools, "PRATG.SIGT", v); c.sigt = l; overflow += of;
            } else if k.eq_ignore_ascii_case(b"OGCRL") {
                c.ogcrl = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"NCF") {
                let (l, of) = enum_of(it, pools, "PRATG.NCF", v); c.ncf = l; overflow += of;
            } else if k.eq_ignore_ascii_case(b"RCX") {
                c.rcx = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"CCI") {
                c.cci = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"NV") {
                let (l, of) = enum_of(it, pools, "PRATG.NV", v); c.nv = l; overflow += of;
            }
        }
        self.enum_overflow += overflow;
        let Some(pln) = pln else { return };
        if op == Op::Rmv {
            self.pending.remove(&pln);
            self.orphans.remove(&pln);
            return;
        }
        self.pending.insert(pln, c);
    }

    /// 批末：把本批 `PRALNK` 写进 `LINK == PLN` 的 PRATG 行（在 `end_batch` 的差分之前）。
    ///
    /// ⚠ 只写**本批 PRATG 真的出现过**的行：`Table::set_cell` 会顺手 `seen.set(row)`，
    /// 若给"本批没出现"的行写列，它就会逃过快照删除，变成 PRATG 列停在上一批的僵尸行
    /// （2026-11-12 自评发现的缺陷②）。这类 `PRALNK` 走 `orphans`，dump 仍能展开（D52）。
    fn flush(&mut self) {
        if self.pending.is_empty() && self.prev.is_empty() && self.batch_tgs.is_empty() {
            return;
        }
        let link_col = col_of("LINK");
        // 本批 PRATG 真的落过的行（TG → 行号）
        let applied_rows: std::collections::HashSet<u32> = self
            .batch_tgs
            .iter()
            .filter_map(|tg| self.core.row_id_of(tg))
            .collect();
        // `LINK 值 → 行号`：一批只建一次（4,376 行量级）
        let mut by_link: std::collections::HashMap<u32, u32> =
            std::collections::HashMap::with_capacity(self.core.alive_count());
        for i in 0..self.core.len() as u32 {
            if !self.core.is_alive(i) {
                continue;
            }
            let Some(r) = self.core.row(i) else { continue };
            if let Some(v) = r.get_col(link_col).as_u32() {
                by_link.insert(v, i);
            }
        }
        let mut cur: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for (pln, c) in std::mem::take(&mut self.pending) {
            match by_link.get(&pln).copied() {
                Some(row) if applied_rows.contains(&row) => {
                    cur.insert(row);
                    self.write_row(row, &c);
                    self.orphans.remove(&pln);
                    self.applied += 1;
                }
                // 没有 PRATG 行引用它，或那一行本批没出现（将被快照删除）→ 记成孤儿
                _ => {
                    self.orphans.insert(pln, c);
                }
            }
        }
        // 上一批有、本批没有 → 清空这 11 列（`PRALNK` 的"缺席=删除"）；
        // 同样只清本批还活着（被 PRATG 落过）的行
        for row in std::mem::take(&mut self.prev) {
            if !cur.contains(&row) && applied_rows.contains(&row) {
                self.write_row(row, &PralnkCols::default());
            }
        }
        self.prev = cur;
        self.batch_tgs.clear();
    }

    fn write_row(&mut self, row: u32, c: &PralnkCols) {
        use crate::vhist::ColVal;
        self.core.set_cell(row, col_of("LNKNAME"), ColVal::sym(c.lnkname));
        self.core.set_cell(row, col_of("SCN"), cv32(c.scn));
        self.core.set_cell(row, col_of("MN"), cv16(c.mn));
        self.core.set_cell(row, col_of("LKS"), cv16(c.lks));
        self.core.set_cell(row, col_of("BINIFID"), cv32(c.binifid));
        self.core.set_cell(row, col_of("SIGT"), cvlab(c.sigt));
        self.core.set_cell(row, col_of("OGCRL"), cv32(c.ogcrl));
        self.core.set_cell(row, col_of("NCF"), cvlab(c.ncf));
        self.core.set_cell(row, col_of("RCX"), cv32(c.rcx));
        self.core.set_cell(row, col_of("CCI"), cv32(c.cci));
        self.core.set_cell(row, col_of("NV"), cvlab(c.nv));
    }

    /// 展开出全部 `PRALNK` 扁平行：`(PLN, 11 列)`；含 `orphans`（D52 不丢）。
    pub fn expand_pralnk(&self) -> Vec<(u32, PralnkCols)> {
        let link_col = col_of("LINK");
        let mut out = Vec::with_capacity(self.core.alive_count() + self.orphans.len());
        for i in 0..self.core.len() as u32 {
            if !self.core.is_alive(i) {
                continue;
            }
            let Some(r) = self.core.row(i) else { continue };
            let Some(pln) = r.get_col(link_col).as_u32() else { continue };
            out.push((
                pln,
                PralnkCols {
                    lnkname: r.lnkname(),
                    scn: r.scn(),
                    mn: r.mn(),
                    lks: r.lks(),
                    binifid: r.binifid(),
                    sigt: r.sigt(),
                    ogcrl: r.ogcrl(),
                    ncf: r.ncf(),
                    rcx: r.rcx(),
                    cci: r.cci(),
                    nv: r.nv(),
                },
            ));
        }
        for (pln, c) in &self.orphans {
            out.push((*pln, *c));
        }
        out
    }

    pub fn set_batch(&mut self, batch: i64, _it: &mut Interner, _pools: &mut LabelCols) {
        self.flush();
        self.core.set_batch(batch);
    }

    pub fn finish(&mut self, _it: &mut Interner, _pools: &mut LabelCols) {
        self.flush();
        self.core.finish();
    }
}

mml_table! {
    PRATKC, Pratkc,
    key [mn, tg, sc0],
    mn: U16,
    tg: U32,
    tgn: Text,
    sc0: U32,
    ec0: U32,
    slots: U8,
    tid0: U32,
    use_lo: U16,
    use_hi: U16,
    tidpfx: Enum,
}

/// 一条展开后的电路（扁平行形态；dump/对账/未来 SQL 展开共用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatCirc {
    pub mn: u16,
    pub tg: u32,
    pub tgn: Option<Sym>,
    pub sc: u32,
    pub ec: u32,
    pub tid: u32,
    /// `CS`：`true` = USE，`false` = UNU
    pub cs_use: bool,
    pub tidpfx: Option<Label>,
}

/// 把 `PRATKC` 的块行**展开回扁平行**（顺序：行号升序、块内槽号升序）。
///
/// `SLOTS` 决定这一行代表几条电路：满块 32，回退行 1（见文件头 D54）。
pub fn expand(t: &PratkcTable) -> Vec<FlatCirc> {
    let mut out = Vec::with_capacity(t.alive_count() * 32);
    for i in 0..t.len() as u32 {
        if !t.is_alive(i) {
            continue;
        }
        let Some(r) = t.row(i) else { continue };
        let (Some(mn), Some(tg), Some(sc0), Some(ec0), Some(slots), Some(tid0), Some(lo), Some(hi)) = (
            r.mn(),
            r.tg(),
            r.sc0(),
            r.ec0(),
            r.slots(),
            r.tid0(),
            r.use_lo(),
            r.use_hi(),
        ) else {
            continue;
        };
        for k in 0..slots as u32 {
            let bit = if k < 16 {
                (lo >> k) & 1
            } else {
                (hi >> (k - 16)) & 1
            };
            out.push(FlatCirc {
                mn,
                tg,
                tgn: r.tgn(),
                sc: sc0 + k,
                ec: ec0 + k,
                tid: tid0 + k,
                cs_use: bit == 1,
                tidpfx: r.tidpfx(),
            });
        }
    }
    out
}

/// 批内聚合：`(MN,TG)` → 该组的全部电路槽位（批末一次性块化）。
#[derive(Debug, Default, Clone)]
struct GrpAcc {
    /// `TGN` 原文（带引号，落行时再 `unquote`）
    tgn: Vec<u8>,
    /// `TIDPFX` 原文
    tidpfx: Vec<u8>,
    /// `(sc, ec, tid, use)`；落行前按 `sc` 升序
    cells: Vec<(u32, u32, u32, bool)>,
}

/// `PRATKC` 的块化外壳（D54）。
#[derive(Default)]
pub struct PratkcShim {
    pub core: PratkcTable,
    pending: std::collections::HashMap<(u16, u32), GrpAcc>,
    /// 本批收了多少条扁平行（对账：应等于原始 PRATKC 行数）
    seen: u64,
    /// 形态不符、回退成逐条行的组数（验收门断言 0）
    exceptions: u64,
    /// 不是声明参数的参数数（应为 0；非 0 说明下面漏了列）
    unknown: u64,
    /// `CS` 同一位出现第二种词形的次数（应为 0）
    cs_odd: u64,
    /// `CS` 的两个词的原文（`[UNU, USE]`，首见即记；块里只存 1 bit）
    cs_words: [Option<Vec<u8>>; 2],
}

impl std::ops::Deref for PratkcShim {
    type Target = PratkcTable;
    fn deref(&self) -> &PratkcTable {
        &self.core
    }
}
impl std::ops::DerefMut for PratkcShim {
    fn deref_mut(&mut self) -> &mut PratkcTable {
        &mut self.core
    }
}

impl PratkcShim {
    pub fn new(batch: i64) -> Self {
        let mut core = PratkcTable::new();
        core.set_batch(batch);
        PratkcShim {
            core,
            ..Default::default()
        }
    }

    /// 展开后的扁平行数（对账：应等于原始 PRATKC 行数）。
    pub fn flat_rows(&self) -> usize {
        (0..self.core.len() as u32)
            .filter(|i| self.core.is_alive(*i))
            .filter_map(|i| self.core.row(i))
            .filter_map(|r| r.slots())
            .map(|n| n as usize)
            .sum()
    }

    pub fn exceptions(&self) -> u64 {
        self.exceptions
    }

    pub fn unknown_params(&self) -> u64 {
        self.unknown
    }

    /// 本批收了多少条扁平行（对账：应等于原始 PRATKC 行数）。
    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// `CS` 同一位出现第二种词形的次数（块里只有 1 bit，词形只能记首见的那一个）。
    pub fn cs_odd(&self) -> u64 {
        self.cs_odd
    }

    /// `CS` 的某个词在原文里怎么写（块里只留 1 bit，导出时还原词形）。
    pub fn cs_word(&self, use_: bool) -> Option<&[u8]> {
        self.cs_words[use_ as usize].as_deref()
    }

    /// 收一条扁平行：`MN/TG/TGN/SC/EC/TID/CS/TIDPFX`。
    pub fn apply(&mut self, op: Op, body: &[u8], _it: &mut Interner, _pools: &mut LabelCols) {
        let mut mn = None;
        let mut tg = None;
        let mut tgn: Option<&[u8]> = None;
        let mut sc = None;
        let mut ec = None;
        let mut tid = None;
        let mut cs: Option<&[u8]> = None;
        let mut tidpfx: Option<&[u8]> = None;
        let mut unknown = 0u64;
        for (k, v) in Params::new(body) {
            if k.eq_ignore_ascii_case(b"MN") {
                mn = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"TG") {
                tg = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"TGN") {
                tgn = Some(v);
            } else if k.eq_ignore_ascii_case(b"SC") {
                sc = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"EC") {
                ec = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"TID") {
                tid = crate::frombytes::parse_u32(v);
            } else if k.eq_ignore_ascii_case(b"CS") {
                cs = Some(v);
            } else if k.eq_ignore_ascii_case(b"TIDPFX") {
                tidpfx = Some(v);
            } else {
                unknown += 1;
            }
        }
        self.unknown += unknown;
        // 键三件（MN/TG/SC）缺一个就没法定位槽位：丢弃。**不静默**——
        // 少一条电路就会让展开行数 < 140,032，值级对拍那一段立刻失败。
        let (Some(mn), Some(tg), Some(sc)) = (mn, tg, sc) else {
            return;
        };
        // EC 缺省 = SC（实测 140,032/140,032 如此；缺省语义与 oracle 的 `-` 无关，
        // 因为 oracle 只在「两边都不写」时才会输出 `-`，而这里 EC 永远写在原文里）
        let ec = ec.unwrap_or(sc);
        let tid = tid.unwrap_or(sc);
        // `CS` 只有 USE/UNU 两个词（实测 140,032/140,032）；块里只留 1 bit
        let use_ = !matches!(
            cs.map(crate::frombytes::unquote),
            Some(w) if w.eq_ignore_ascii_case(b"UNU")
        );
        if let Some(w) = cs {
            let slot = &mut self.cs_words[use_ as usize];
            let w = crate::frombytes::unquote(w);
            match slot {
                None => *slot = Some(w.to_vec()),
                Some(prev) if prev.as_slice() == w => {}
                // 同一个 bit 出现第二种词形：不静默改，记数（导出会用首见词形）
                Some(_) => self.cs_odd += 1,
            }
        }
        let g = self.pending.entry((mn, tg)).or_default();
        if tgn.is_some() {
            g.tgn = tgn.unwrap_or_default().to_vec();
        }
        if tidpfx.is_some() {
            g.tidpfx = tidpfx.unwrap_or_default().to_vec();
        }
        if op == Op::Rmv {
            g.cells.retain(|c| c.0 != sc);
            return;
        }
        self.seen += 1;
        g.cells.push((sc, ec, tid, use_));
    }

    /// 批末：每个 `(MN,TG)` 落成若干 PCM 块行（形态不符则整组回退成 `SLOTS=1` 的逐条行）。
    fn flush(&mut self, it: &mut Interner, pools: &mut LabelCols) {
        if self.pending.is_empty() {
            return;
        }
        let pfx_pool = pools.register("PRATKC.TIDPFX");
        let mut groups: Vec<((u16, u32), GrpAcc)> = std::mem::take(&mut self.pending).into_iter().collect();
        groups.sort_unstable_by_key(|(k, _)| *k);
        for ((mn, tg), mut acc) in groups {
            if acc.cells.is_empty() {
                continue;
            }
            acc.cells.sort_unstable_by_key(|c| c.0);
            let tgn = it.intern(crate::frombytes::unquote(&acc.tgn));
            let tidpfx = if acc.tidpfx.is_empty() {
                None
            } else {
                let sym = it.intern(crate::frombytes::unquote(&acc.tidpfx));
                pools.label_of(pfx_pool, sym)
            };
            // `EC - SC` 在块内恒定（`blockify` 已校验）；槽 0 的差值就是整行的步长
            let ec_d = acc.cells[0].1 as i64 - acc.cells[0].0 as i64;
            match blockify(mn, &acc.cells) {
                Some(blocks) => {
                    for b in blocks {
                        let sc0 = b.sc0 as u32;
                        let ec0 = (sc0 as i64 + ec_d) as u32;
                        self.push_row(it, mn, tg, tgn, sc0, ec0, 32, b.tid0 as u32, b.use_mask, tidpfx);
                    }
                }
                None => {
                    self.exceptions += 1;
                    for (sc, ec, tid, use_) in acc.cells.iter().copied() {
                        let mask = if use_ { 1u32 } else { 0 };
                        self.push_row(it, mn, tg, tgn, sc, ec, 1, tid, mask, tidpfx);
                    }
                }
            }
        }
    }

    /// 落一个块行（`slots` = 这行代表几条电路：满块 32 / 回退行 1）。
    #[allow(clippy::too_many_arguments)]
    fn push_row(
        &mut self,
        it: &Interner,
        mn: u16,
        tg: u32,
        tgn: Sym,
        sc0: u32,
        ec0: u32,
        slots: u8,
        tid0: u32,
        use_mask: u32,
        tidpfx: Option<Label>,
    ) {
        let p = PratkcPending {
            mn: Some(mn),
            tg: Some(tg),
            tgn: Some(tgn),
            sc0: Some(sc0),
            ec0: Some(ec0),
            slots: Some(slots),
            tid0: Some(tid0),
            use_lo: Some((use_mask & 0xffff) as u16),
            use_hi: Some((use_mask >> 16) as u16),
            tidpfx,
            ..Default::default()
        };
        self.core.apply_pending(Op::Add, p, it);
    }

    pub fn set_batch(&mut self, batch: i64, it: &mut Interner, pools: &mut LabelCols) {
        self.flush(it, pools);
        self.core.set_batch(batch);
    }

    pub fn finish(&mut self, it: &mut Interner, pools: &mut LabelCols) {
        self.flush(it, pools);
        self.core.finish();
    }
}

/// 一组电路槽位 → PCM 块（`None` = 形态不符，整组回退）。
fn blockify(mn: u16, cells: &[(u32, u32, u32, bool)]) -> Option<Vec<crate::circuit::PcmBlock>> {
    if cells.is_empty() || !cells.len().is_multiple_of(32) {
        return None;
    }
    let sc0 = cells[0].0;
    let tid_d = cells[0].2 as i64 - sc0 as i64;
    let ec_d = cells[0].1 as i64 - sc0 as i64;
    for (i, c) in cells.iter().enumerate() {
        if c.0 != sc0 + i as u32 {
            return None; // SC 不连续
        }
        if c.2 as i64 - c.0 as i64 != tid_d {
            return None; // TID 与 SC 不同差
        }
        if c.1 as i64 - c.0 as i64 != ec_d {
            return None; // EC 与 SC 不同差
        }
        if c.0 > u16::MAX as u32 || c.1 > u16::MAX as u32 || c.2 > u16::MAX as u32 {
            return None; // 超出 `PcmBlock` 的 u16 表示（回退到逐条行，仍然无损）
        }
    }
    let mut out = Vec::with_capacity(cells.len() / 32);
    for ch in cells.chunks(32) {
        let mut mask = 0u32;
        for (i, c) in ch.iter().enumerate() {
            if c.3 {
                mask |= 1 << i;
            }
        }
        out.push(crate::circuit::PcmBlock {
            mn,
            sc0: ch[0].0 as u16,
            tid0: ch[0].2 as u16,
            use_mask: mask,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use crate::sym::Interner;
    use crate::{scan::Op, LabelCols};

    fn add_line(shim: &mut super::PratkcShim, it: &mut Interner, pools: &mut LabelCols, s: &str) {
        shim.apply(Op::Add, s.as_bytes(), it, pools);
    }

    fn tg_add(shim: &mut super::PratgShim, it: &mut Interner, pools: &mut LabelCols, s: &str) {
        shim.apply_tg(Op::Add, s.as_bytes(), it, pools);
    }

    fn lnk_add(shim: &mut super::PratgShim, it: &mut Interner, pools: &mut LabelCols, s: &str) {
        shim.apply_lnk(Op::Add, s.as_bytes(), it, pools);
    }

    #[test]
    fn pratg_keeps_base_column_numbers_after_merging_pralnk() {
        // 前 25 列的列号必须不动（refs_gen 的 from_col 依赖它）
        // A3 起 `CDEF` 后面多了一个伴生列 `CDEF_NUM`，所以 CDEF 之后的列号整体 +1
        assert_eq!(super::col_of("LINK"), 4, "CDEF 之前的列号不动");
        assert_eq!(super::col_of("CDEF"), 5);
        assert_eq!(super::col_of("CDEF_NUM"), 6, "A3 的伴生列紧跟在 CDEF 后面");
        assert_eq!(super::col_of("TGMS"), 25, "CDEF 之后的列号整体 +1");
        assert_eq!(super::col_of("LNKNAME"), 26, "并入的 11 列追加在末尾");
        assert_eq!(super::col_of("NV"), 36);
    }

    #[test]
    fn pralnk_lands_on_the_pratg_row_after_batch_end() {
        // 实测块序：PRALNK 先到、PRATG 后到（6/6 文件）
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = super::PratgShim::new(100);
        lnk_add(
            &mut s,
            &mut it,
            &mut pools,
            "PLN=1,LNKNAME=\"A-1\",SCN=16,MN=1009,LKS=0,BINIFID=1001,SIGT=NET,OGCRL=2,NCF=Any_CLI,RCX=3,CCI=4,NV=ITU",
        );
        tg_add(&mut s, &mut it, &mut pools, "TG=7,TGN=\"T7\",LINK=1,CDEF=K'28811399,CSC=11");
        assert!(s.core.get(&7).unwrap().lnkname().is_none(), "批末之前不写");
        s.finish(&mut it, &mut pools);

        let r = s.core.get(&7).expect("PRATG 行");
        assert_eq!(r.lnkname().map(|x| it.decode(x)), Some("A-1".to_string()));
        assert_eq!(r.mn(), Some(1009));
        assert_eq!(r.cci(), Some(4));
        assert_eq!(r.scn(), Some(16));
        assert_eq!(s.applied(), 1);
        // 展开回扁平行：1 行、PLN = LINK
        let flat = s.expand_pralnk();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].0, 1);
        assert_eq!(flat[0].1.mn, Some(1009));
        assert!(s.orphan_count() == 0);
    }

    #[test]
    fn pralnk_without_any_pratg_reference_is_kept_as_orphan() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = super::PratgShim::new(100);
        lnk_add(&mut s, &mut it, &mut pools, "PLN=347,LNKNAME=\"orphan\",MN=1001");
        tg_add(&mut s, &mut it, &mut pools, "TG=9,TGN=\"T9\",LINK=999");
        s.finish(&mut it, &mut pools);
        assert_eq!(s.orphan_count(), 1, "被引用的才算并入；孤儿要留（D52）");
        let flat = s.expand_pralnk();
        assert_eq!(flat.len(), 2, "1 条并入 + 1 条孤儿");
        assert!(flat.iter().any(|(pln, c)| *pln == 347 && c.lnkname.is_some()));
    }

    #[test]
    fn pralnk_columns_are_cleared_when_the_link_disappears_next_batch() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = super::PratgShim::new(100);
        lnk_add(&mut s, &mut it, &mut pools, "PLN=1,LNKNAME=\"A\",MN=5");
        tg_add(&mut s, &mut it, &mut pools, "TG=7,TGN=\"T7\",LINK=1");
        s.set_batch(200, &mut it, &mut pools);
        assert!(s.core.get(&7).unwrap().lnkname().is_some(), "第一批已落到行上");
        // 第二批：PRATG 还在（MOD），但本批没有 PLN=1 的 PRALNK → 11 列清空
        s.apply_tg(Op::Mod, b"TG=7,TGN=\"T7b\"", &mut it, &mut pools);
        s.finish(&mut it, &mut pools);
        let r = s.core.get(&7).unwrap();
        assert_eq!(r.tgn().map(|x| it.decode(x)), Some("T7b".to_string()));
        assert!(r.lnkname().is_none(), "PRALNK 缺席 = 这 11 列被清空（快照语义）");
        assert!(r.mn().is_none());
    }

    #[test]
    fn pralnk_does_not_resurrect_a_pratg_row_missing_from_this_batch() {
        // 缺陷②：set_cell 会标记 seen；如果给"本批没有 PRATG 行"的行写列，
        // 它就会逃过快照删除（PRATG 列停在上一批的僵尸行）。
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = super::PratgShim::new(100);
        s.set_snapshot(true);
        tg_add(&mut s, &mut it, &mut pools, "TG=7,TGN=\"T7\",LINK=1");
        lnk_add(&mut s, &mut it, &mut pools, "PLN=1,LNKNAME=\"A\",MN=5");
        s.set_batch(200, &mut it, &mut pools);
        assert_eq!(
            s.core.get(&7).unwrap().lnkname().map(|x| it.decode(x)),
            Some("A".to_string()),
            "第一批正常并入"
        );
        assert_eq!(s.orphan_count(), 0);

        // 第二批：本批**没有** PRATG 行，只有 PRALNK → 行必须被快照删除
        lnk_add(&mut s, &mut it, &mut pools, "PLN=1,LNKNAME=\"B\",MN=6");
        s.finish(&mut it, &mut pools);
        assert_eq!(s.core.alive_count(), 0, "PRATG 本批没出现 → 必须删掉");
        assert_eq!(s.orphan_count(), 1, "那条 PRALNK 进孤儿（D52 不丢）");
        let flat = s.expand_pralnk();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].0, 1);
        assert_eq!(flat[0].1.lnkname.map(|x| it.decode(x)), Some("B".to_string()));
    }
    #[test]
    fn thirty_two_circuits_become_one_block_and_expand_back() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::PratkcShim::new(100);
        for sc in 0..32u32 {
            let line = format!(
                "MN=1000,TG=2251,TGN=\"A\",SC={sc},EC={sc},TID=\"{}\",CS={},TIDPFX=Index1",
                49152 + sc,
                if sc % 3 == 0 { "UNU" } else { "USE" }
            );
            add_line(&mut t, &mut it, &mut pools, &line);
        }
        assert_eq!(t.core.len(), 0, "flush 之前不建行");
        t.finish(&mut it, &mut pools);
        assert_eq!(t.core.len(), 1, "32 条电路 = 1 个 PCM 块");
        assert_eq!(t.flat_rows(), 32, "展开后仍 32 条");
        assert_eq!(t.exceptions(), 0);

        let flat = super::expand(&t.core);
        assert_eq!(flat.len(), 32);
        for (i, f) in flat.iter().enumerate() {
            assert_eq!((f.mn, f.tg, f.sc, f.ec, f.tid), (1000, 2251, i as u32, i as u32, 49152 + i as u32));
            assert_eq!(f.cs_use, i % 3 != 0);
        }
        assert_eq!(t.cs_word(true), Some(&b"USE"[..]));
        assert_eq!(t.cs_word(false), Some(&b"UNU"[..]));
    }

    #[test]
    fn irregular_group_falls_back_to_one_row_per_circuit_without_losing_values() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::PratkcShim::new(100);
        // 不连续 + 非 32 倍数 + EC ≠ SC → 整组回退
        for (sc, ec, tid) in [(0u32, 0u32, 10u32), (2, 5, 12), (5, 9, 15)] {
            let line = format!("MN=9,TG=7,TGN=\"B\",SC={sc},EC={ec},TID={tid},CS=USE,TIDPFX=Index1");
            add_line(&mut t, &mut it, &mut pools, &line);
        }
        t.finish(&mut it, &mut pools);
        assert_eq!(t.core.len(), 3, "回退成 3 条逐条行");
        assert_eq!(t.exceptions(), 1);
        assert_eq!(t.flat_rows(), 3, "回退后条数不变");
        let flat = super::expand(&t.core);
        // EC ≠ SC 也必须逐字段还原（回退行的 ec0 就是它自己的 EC）
        let got: Vec<(u32, u32, u32, bool)> =
            flat.iter().map(|f| (f.sc, f.ec, f.tid, f.cs_use)).collect();
        assert_eq!(got, vec![(0, 0, 10, true), (2, 5, 12, true), (5, 9, 15, true)]);
    }

    #[test]
    fn all_unu_block_expands_to_thirty_two_unu_rows() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::PratkcShim::new(100);
        for sc in 0..32u32 {
            let line = format!("MN=1,TG=2,TGN=\"C\",SC={sc},EC={sc},TID={sc},CS=UNU,TIDPFX=Index1");
            add_line(&mut t, &mut it, &mut pools, &line);
        }
        t.finish(&mut it, &mut pools);
        assert_eq!(t.core.len(), 1);
        let flat = super::expand(&t.core);
        assert_eq!(flat.len(), 32, "全 UNU 的块也是 32 条（不能被当成 1 条回退行）");
        assert!(flat.iter().all(|f| !f.cs_use));
    }

    #[test]
    fn blocks_go_through_snapshot_delete_on_next_batch() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::PratkcShim::new(100);
        t.set_snapshot(true);
        for sc in 0..32u32 {
            let line = format!("MN=1,TG=2,TGN=\"C\",SC={sc},EC={sc},TID={sc},CS=USE,TIDPFX=Index1");
            add_line(&mut t, &mut it, &mut pools, &line);
        }
        // 第二批：这个 TG 消失了 → 快照删除
        t.set_batch(200, &mut it, &mut pools);
        t.finish(&mut it, &mut pools);
        assert_eq!(t.core.alive_count(), 0, "本批未出现 = 删除（D32）");
        assert_eq!(t.core.len(), 1, "行号稳定：行还在，只是墓碑");
    }

    #[test]
    fn blockify_rejects_non_uniform_tid_step() {
        // 32 条连续 SC，但 TID 与 SC 不同差 → 不块化
        let cells: Vec<(u32, u32, u32, bool)> = (0..32u32)
            .map(|i| (i, i, if i == 31 { 999 } else { 100 + i }, true))
            .collect();
        assert!(super::blockify(1, &cells).is_none());
    }
}

