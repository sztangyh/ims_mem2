//! N7 电路明细的**稀疏**表示（D53 步骤 2 / D57）。
//!
//! 依据（`tools/probe_n7.py` / `probe_n7_extra.py`，6 文件实测）：
//! - `N7TKC` 4,448 行、29 个 `(NE,TG)` 组，每组 32/64/128/192/352/672 条（全是 32 的倍数），
//!   但 `SC` **稀疏**（25/29 组连续，其余有洞），`SCIC` 有 1,344 种取值 → 位图表达不了；
//! - `EC == SC` 逐条成立、`TGN` 与 `N7TG.TGN` 4,448/4,448 相同、`CS ∈ {USE,UNU}`、`SCF` 是布尔；
//! - 3 个**未声明**参数 `MD=FRE` / `SEN=39999` / `TIDPFX=INDEX1` 在 **29/29 组内恒定**（全局也各只有 1 种）。
//!
//! 所以一条电路 = `Circuit{mn,sc,ec,tid,scic,flags}`（11 B），按 TG 聚成一段，
//! 句柄 `(circ_start, circ_len)` 落在 `N7TG` 行的**末尾两列**（`port_schema.py` 的 `EXTRA_COLS`）。
//!
//! # 三个层次（看清了才不会算错行数）
//!
//! | 层 | 内容 | 谁在用 |
//! | --- | --- | --- |
//! | `entries` + `spans` | **历史**：只追加，段 `start` 永不复用 | `as_of` / `hist_N7TG` 的句柄 |
//! | `live` | **当前态**：每 TG 恰好一个段 | 视图的行空间（`len()` / `col_*` / dump / SQL） |
//! | `heads` | 组头：`TGN` + `MD/SEN/TIDPFX` + `CS` 词形 | 视图的 `TGN`/`CS` 列、`lifted` 台账 |
//!
//! **`live` 与 `entries` 必须分开**：如果视图直接摊平整个 arena，同一个网元导两批
//! （哪怕内容一模一样）就会在 `cur_N7TKC` 里出现两份行 —— 2026-11-12 自评发现的缺陷①：
//! 4 文件那组数据里 AGCF52 报 1,536 行，应为 768。现在批末做**内容寻址复用**
//! （和 D50 的"方案池同内容复用 idx"同一手法）：这一批的槽位序列与上一段完全相同
//! → 直接复用旧段，不追加历史、不重复计行。
//!
//! # 三个"不丢信息"的兜底（D52）
//!
//! 1. `Circuit` 装不下的东西（`TGN` + `MD/SEN/TIDPFX` + `CS` 词形）放**组头**；
//!    组内不恒定 → 计 `extra_odd`，保留首见值。
//! 2. `TG` 在 `N7TKC` 里出现但 `N7TG` 没有对应行 → 段照样进 arena（数据不丢），
//!    只是没有句柄指过来；`counts().no_handle` 会计数。
//! 3. 任何装不进 `Circuit` 的行（`u16` 越界等）→ 计 `overflow`，验收门断言 0。
//!
//! # 为什么还要实现 [`TableApi`]
//!
//! `N7TKC` 从"长尾表"变成"arena + 视图"之后，如果直接从表册里消失，
//! SQL 投影 / 长尾对拍 / Ref 都会失去它。所以 [`N7Circuits`] 实现 `TableApi`
//! （**只读视图**：行号 = `live` 摊平后的下标），`NeStore::tables()` 照样把 `N7TKC` 报出去。
//! 版本链是空的：N7TKC 的历史走 `hist_N7TG` 的 `CIRC_START/CIRC_LEN`（见上）。

use std::collections::{BTreeMap, HashMap};

use crate::circuit::Circuit;
use crate::label::LabelCols;
use crate::mml_table;
use crate::scan::{Op, Params};
use crate::sym::{Interner, Sym};
use crate::table::{TableApi, TableHooks};
use crate::vhist::HistRow;

// 列契约（列名/列序/键）与原长尾声明**完全一致**：`tools/gen_refs.py` 按它算列号，
// 视图按它报 `col_meta`，所以 N7TKC.MN→MODULE.MID / N7TKC.TG→N7TG.TG 两条边不用改。
mml_table! {
    N7TKC, N7Tkc,
    key [mn, sc],
    mn: U16,
    sc: U16,
    cs: Text,
    ec: U16,
    scf: Bool,
    scic: U16,
    tg: U16,
    tgn: Text,
    tid: U16,
}

/// 一个 TG 的电路段：`entries[start .. start+len]`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub tg: u16,
    pub start: u32,
    pub len: u16,
    pub tgn: Option<Sym>,
}

/// `Circuit` 装不下的、实测组内恒定的东西（见文件头 D52 第 1 条）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GroupHead {
    pub tgn: Option<Sym>,
    /// 未声明参数 `MD` 的原文
    pub md: Vec<u8>,
    /// 未声明参数 `SEN` 的原文
    pub sen: Vec<u8>,
    /// 未声明参数 `TIDPFX` 的原文
    pub tidpfx: Vec<u8>,
    /// `CS` 两个词形 `[UNU, USE]`（驻留后的符号；不同拼写 = 不同符号）
    pub cs_words: [Option<Sym>; 2],
}

impl GroupHead {
    fn params_present(&self) -> usize {
        [self.md.is_empty(), self.sen.is_empty(), self.tidpfx.is_empty()]
            .iter()
            .filter(|e| !**e)
            .count()
    }
}

/// 计数器（对账/验收门用；除 `seen`/`applied`/`reused` 外都应为 0）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct N7Counts {
    /// 收了多少条 `N7TKC` 扁平行（**逻辑行**，多批累加）
    pub seen: u64,
    /// 真正追加进 arena 的电路条数（复用旧段的不计）
    pub applied: u64,
    /// 因为"内容与上一段完全相同"而复用的段数
    pub reused: u64,
    /// `TG` 没有 `N7TG` 行的**段**数（数据仍在 arena 里，只是没句柄）
    pub no_handle: u64,
    /// `CS` 同一位出现第二种词形
    pub cs_odd: u64,
    /// `MD/SEN/TIDPFX` 组内不恒定（保留首见值）
    pub extra_odd: u64,
    /// 装不进 `Circuit` 的行（u16 越界等）
    pub overflow: u64,
}

#[derive(Debug, Default)]
struct PendingGrp {
    head: GroupHead,
    cells: Vec<Circuit>,
}

/// N7 电路的唯一存储：稀疏条目 + 每 TG 一段。
#[derive(Debug, Default)]
pub struct N7Circuits {
    /// 历史：只追加（`as_of` 靠"段 start 永不复用"成立）
    entries: Vec<Circuit>,
    spans: Vec<Span>,
    /// 当前态：每 TG 一个段（视图的行空间）
    live: Vec<Span>,
    /// `live` 的前缀和（长度 = live.len() + 1）；视图行号二分用
    live_prefix: Vec<u32>,
    /// `tg → spans 下标`（内容寻址复用用）
    last: HashMap<u16, usize>,
    /// 本批：`tg → 待落盘的电路`
    pending: BTreeMap<u16, PendingGrp>,
    /// 上一批写过句柄的 TG（本批没有 → 把 `circ_len` 清 0）
    prev_tgs: Vec<u16>,
    /// 每 TG 的组头（导出/对账用；29 组量级）
    heads: HashMap<u16, GroupHead>,
    counts: N7Counts,
    lifted_present: usize,
}

impl N7Circuits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn counts(&self) -> N7Counts {
        self.counts
    }

    /// 历史段（只追加；`as_of` 用）。
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// 当前态的段（每 TG 一个；视图行空间 = 这些段摊平）。
    pub fn live_spans(&self) -> &[Span] {
        &self.live
    }

    /// arena 里的条目（历史全量）。
    pub fn entries(&self) -> &[Circuit] {
        &self.entries
    }

    /// 某 TG 的组头（`TGN/MD/SEN/TIDPFX/CS 词形`）。
    pub fn head(&self, tg: u16) -> Option<&GroupHead> {
        self.heads.get(&tg)
    }

    /// 视图行数 = 当前态的电路条数。
    pub fn expanded_len(&self) -> usize {
        self.view_len() as usize
    }

    fn view_len(&self) -> u32 {
        *self.live_prefix.last().unwrap_or(&0)
    }

    /// 收一条 `N7TKC` 扁平行（`ADD`/`MOD`/`SET`/`RMV`）。
    pub fn apply_line(&mut self, op: Op, body: &[u8], it: &mut Interner, _pools: &mut LabelCols) {
        let mut mn = None;
        let mut sc = None;
        let mut ec = None;
        let mut tid = None;
        let mut scic = None;
        let mut tg = None;
        let mut tgn: Option<&[u8]> = None;
        let mut cs: Option<&[u8]> = None;
        let mut scf: Option<bool> = None;
        let mut md: Option<&[u8]> = None;
        let mut sen: Option<&[u8]> = None;
        let mut tidpfx: Option<&[u8]> = None;
        for (k, v) in Params::new(body) {
            if k.eq_ignore_ascii_case(b"MN") {
                mn = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"SC") {
                sc = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"EC") {
                ec = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"TID") {
                tid = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"SCIC") {
                scic = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"TG") {
                tg = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"TGN") {
                tgn = Some(v);
            } else if k.eq_ignore_ascii_case(b"CS") {
                cs = Some(v);
            } else if k.eq_ignore_ascii_case(b"SCF") {
                scf = crate::frombytes::parse_bool(v);
            } else if k.eq_ignore_ascii_case(b"MD") {
                md = Some(v);
            } else if k.eq_ignore_ascii_case(b"SEN") {
                sen = Some(v);
            } else if k.eq_ignore_ascii_case(b"TIDPFX") {
                tidpfx = Some(v);
            }
        }
        // 键三件（MN/SC/TG）缺一个就定位不了槽位；少一条会让展开行数对不上，值级对拍立刻失败
        let (Some(mn), Some(sc), Some(tg)) = (mn, sc, tg) else {
            return;
        };
        let g = self.pending.entry(tg).or_default();
        // ---- 组头：TGN + 3 个未声明参数 + CS 两个词形 ----
        if let Some(v) = tgn {
            let s = it.intern(crate::frombytes::unquote(v));
            if g.head.tgn.is_none() {
                g.head.tgn = Some(s);
            }
        }
        for (slot, v) in [(&mut g.head.md, md), (&mut g.head.sen, sen), (&mut g.head.tidpfx, tidpfx)] {
            if let Some(v) = v {
                let raw = crate::frombytes::unquote(v).to_vec();
                if slot.is_empty() {
                    *slot = raw;
                } else if *slot != raw {
                    self.counts.extra_odd += 1;
                }
            }
        }
        let use_ = !matches!(cs.map(crate::frombytes::unquote), Some(w) if w.eq_ignore_ascii_case(b"UNU"));
        if let Some(raw) = cs {
            let raw = it.intern(crate::frombytes::unquote(raw));
            let slot = &mut g.head.cs_words[use_ as usize];
            match slot {
                None => *slot = Some(raw),
                Some(prev) if *prev == raw => {}
                Some(_) => self.counts.cs_odd += 1,
            }
        }
        let mut flags = 0u8;
        if use_ {
            flags |= Circuit::F_USE;
        }
        if scf == Some(true) {
            flags |= Circuit::F_SCF;
        }
        if op == Op::Rmv {
            g.cells.retain(|c| !(c.mn == mn && c.sc == sc));
            return;
        }
        self.counts.seen += 1;
        g.cells.push(Circuit {
            mn,
            sc,
            ec: ec.unwrap_or(sc),
            tid: tid.unwrap_or(sc),
            scic: scic.unwrap_or(0),
            flags,
        });
    }

    /// 批末：每个 TG 落一段，返回 `(要写回 N7TG 的句柄, 要清 0 的 TG)`。
    ///
    /// 内容寻址：这一批的槽位序列与"该 TG 上一段"完全相同时直接复用旧段
    /// —— 既不追加历史、也不重复计行（缺陷①的修复）。
    pub fn flush(&mut self) -> (Vec<(u16, u32, u16)>, Vec<u16>) {
        let mut out: Vec<(u16, u32, u16)> = Vec::with_capacity(self.pending.len());
        for (tg, mut g) in std::mem::take(&mut self.pending) {
            if g.cells.is_empty() {
                continue;
            }
            // 同一 TG 内按 (sc, ec) 排序，导出顺序稳定
            g.cells.sort_unstable_by_key(|c| (c.sc, c.ec));
            if g.cells.len() > u16::MAX as usize {
                self.counts.overflow += 1;
                continue;
            }
            self.lifted_present = self.lifted_present.max(g.head.params_present());
            let head = g.head;
            // 和这个 TG 上一段完全相同（含 TGN）→ 复用
            let reuse = match self.last.get(&tg).copied() {
                Some(i) => {
                    let s = self.spans[i];
                    (s.len as usize == g.cells.len()
                        && s.tgn == head.tgn
                        && self.entries[s.start as usize..s.start as usize + s.len as usize]
                            == g.cells[..])
                        .then_some(s)
                }
                None => None,
            };
            let span = match reuse {
                Some(s) => {
                    self.counts.reused += 1;
                    s
                }
                None => {
                    let start = self.entries.len() as u32;
                    let len = g.cells.len() as u16;
                    self.entries.extend_from_slice(&g.cells);
                    let s = Span {
                        tg,
                        start,
                        len,
                        tgn: head.tgn,
                    };
                    self.spans.push(s);
                    self.last.insert(tg, self.spans.len() - 1);
                    self.counts.applied += len as u64;
                    s
                }
            };
            self.heads.insert(tg, head);
            out.push((tg, span.start, span.len));
        }
        let new_tgs: Vec<u16> = out.iter().map(|(tg, _, _)| *tg).collect();
        let stale: Vec<u16> = self
            .prev_tgs
            .iter()
            .copied()
            .filter(|t| !new_tgs.contains(t))
            .collect();
        // 上一批有段、本批没有的 TG：从当前态里摘掉（它的电路没了）
        for t in &stale {
            self.last.remove(t);
        }
        self.prev_tgs = new_tgs;
        self.rebuild_live();
        (out, stale)
    }

    /// 由 `last` 重算当前态（每 TG 一段）与前缀和。
    fn rebuild_live(&mut self) {
        let mut v: Vec<Span> = self.last.values().map(|&i| self.spans[i]).collect();
        v.sort_unstable_by_key(|s| s.start);
        self.live = v;
        self.live_prefix.clear();
        let mut acc = 0u32;
        for s in &self.live {
            self.live_prefix.push(acc);
            acc += s.len as u32;
        }
        self.live_prefix.push(acc);
    }

    /// 记一个"没有 N7TG 行可指"的段（数据仍在 arena 里，只是没句柄）。
    pub fn mark_no_handle(&mut self) {
        self.counts.no_handle += 1;
    }

    /// arena 自身占用（报告用）。
    pub fn bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<Circuit>()
            + self.spans.capacity() * std::mem::size_of::<Span>()
            + self
                .heads
                .values()
                .map(|h| h.md.len() + h.sen.len() + h.tidpfx.len() + 64)
                .sum::<usize>()
    }

    /// 视图行号 → (当前态的段, 电路)。
    fn locate(&self, row: u32) -> Option<(&Span, &Circuit)> {
        if row >= self.view_len() {
            return None;
        }
        let i = match self.live_prefix.binary_search(&row) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let s = *self.live.get(i)?;
        let off = self.live_prefix[i];
        if row >= off + s.len as u32 {
            return None;
        }
        let k = (row - off) as usize;
        self.entries
            .get(s.start as usize + k)
            .map(|c| (&self.live[i], c))
    }

    fn col_index(name: &str) -> u16 {
        <N7TkcHooks as TableHooks>::COL_NAMES
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("N7TKC 声明里应有 {name} 列")) as u16
    }
}

impl TableApi for N7Circuits {
    fn moc(&self) -> &'static str {
        "N7TKC"
    }
    /// 只读视图：写入统一走 [`N7Circuits::apply_line`]（`import.rs` 直接调）。
    fn apply(&mut self, _op: Op, _body: &[u8], _it: &mut Interner, _pools: &mut LabelCols) {}
    fn set_batch(&mut self, _batch: i64) {}
    fn set_snapshot(&mut self, _on: bool) {}
    fn finish(&mut self) {}
    fn len(&self) -> usize {
        self.view_len() as usize
    }
    fn alive_count(&self) -> usize {
        self.view_len() as usize
    }
    fn versions(&self) -> usize {
        // 历史走 `hist_N7TG` 的 CIRC_START/CIRC_LEN（见文件头）
        0
    }
    fn bytes(&self) -> usize {
        N7Circuits::bytes(self)
    }
    fn col_sym(&self, row: u32, col: u16) -> Option<Sym> {
        let (s, e) = self.locate(row)?;
        if col == Self::col_index("CS") {
            let head = self.heads.get(&s.tg)?;
            head.cs_words[e.is_use() as usize]
        } else if col == Self::col_index("TGN") {
            s.tgn
        } else {
            None
        }
    }
    fn col_u32(&self, row: u32, col: u16) -> Option<u32> {
        let (s, e) = self.locate(row)?;
        if col == Self::col_index("MN") {
            Some(e.mn as u32)
        } else if col == Self::col_index("SC") {
            Some(e.sc as u32)
        } else if col == Self::col_index("EC") {
            Some(e.ec as u32)
        } else if col == Self::col_index("SCF") {
            Some(if e.is_scf() { 1 } else { 0 })
        } else if col == Self::col_index("SCIC") {
            Some(e.scic as u32)
        } else if col == Self::col_index("TG") {
            Some(s.tg as u32)
        } else if col == Self::col_index("TID") {
            Some(e.tid as u32)
        } else {
            None
        }
    }
    fn index_by_sym(&self, col: u16) -> HashMap<Sym, u32> {
        let mut m = HashMap::new();
        for row in 0..self.view_len() {
            if let Some(v) = self.col_sym(row, col) {
                m.entry(v).or_insert(row);
            }
        }
        m
    }
    fn index_by_u32(&self, col: u16) -> HashMap<u32, u32> {
        let mut m = HashMap::new();
        for row in 0..self.view_len() {
            if let Some(v) = self.col_u32(row, col) {
                m.entry(v).or_insert(row);
            }
        }
        m
    }
    fn col_meta(&self) -> (&'static [&'static str], &'static [u8]) {
        (
            <N7TkcHooks as TableHooks>::COL_NAMES,
            <N7TkcHooks as TableHooks>::COL_KINDS,
        )
    }
    fn col_groups(&self) -> &'static [(u16, u8)] {
        &[]
    }
    fn group_of(&self, _row: u32, _col: u16) -> Option<&crate::vhist::GroupVal> {
        None
    }
    fn is_alive(&self, row: u32) -> bool {
        row < self.view_len()
    }
    fn key_cols(&self) -> &'static [u16] {
        <N7TkcHooks as TableHooks>::KEY_COLS
    }
    fn hist_rows(&self) -> Vec<HistRow> {
        Vec::new()
    }
    fn lifted_stats(&self) -> (usize, usize, usize, usize, u64) {
        // `MD/SEN/TIDPFX` 三个未声明参数：实测组内恒定 → 计入"常量"；
        // 组内不一致的次数进 `dev`（验收门断言为 0）。
        let p = self.lifted_present;
        (p, p, 0, self.counts.extra_odd as usize, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(s: &mut N7Circuits, it: &mut Interner, pools: &mut LabelCols, line: &str) {
        s.apply_line(Op::Add, line.as_bytes(), it, pools);
    }

    fn four_circuits(s: &mut N7Circuits, it: &mut Interner, pools: &mut LabelCols) {
        for i in 0..4u16 {
            add(
                s,
                it,
                pools,
                &format!(
                    "MN=1000,SC={},EC={},SCIC={},TG=7,TGN=\"T7\",TID={},CS=USE,SCF=TRUE,MD=FRE,SEN=39999,TIDPFX=INDEX1",
                    608 + i,
                    608 + i,
                    9000 + i,
                    100 + i
                ),
            );
        }
    }

    #[test]
    fn circuits_group_into_one_span_and_expand_back() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = N7Circuits::new();
        four_circuits(&mut s, &mut it, &mut pools);
        assert_eq!(s.len(), 0, "flush 之前不落 arena");
        let (spans, stale) = s.flush();
        assert_eq!(spans, vec![(7, 0, 4)]);
        assert!(stale.is_empty());
        assert_eq!(s.len(), 4);
        assert_eq!(s.counts().applied, 4);
        assert_eq!(s.counts().no_handle, 0);
        // 视图口径
        assert_eq!(s.col_u32(0, N7Circuits::col_index("SC")), Some(608));
        assert_eq!(s.col_u32(0, N7Circuits::col_index("TG")), Some(7));
        assert_eq!(s.col_u32(0, N7Circuits::col_index("SCIC")), Some(9000));
        assert_eq!(s.col_u32(0, N7Circuits::col_index("SCF")), Some(1));
        assert_eq!(s.col_u32(3, N7Circuits::col_index("TID")), Some(103));
        // 组头里的 3 个未声明参数都在
        let h = s.head(7).expect("组头");
        assert_eq!(h.md, b"FRE");
        assert_eq!(h.sen, b"39999");
        assert_eq!(h.tidpfx, b"INDEX1");
        // lifted 台账：3 个参数都是常量
        assert_eq!(s.lifted_stats(), (3, 3, 0, 0, 0));
        // 句柄指向的切片必须属于该 TG（内部不变量）
        for sp in s.spans() {
            for e in &s.entries()[sp.start as usize..sp.start as usize + sp.len as usize] {
                assert_eq!(e.mn, 1000);
            }
        }
    }

    #[test]
    fn identical_second_batch_reuses_the_span_without_duplicating_rows() {
        // 缺陷①：同一网元导两批（内容相同）不能在视图里变成 8 行
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = N7Circuits::new();
        four_circuits(&mut s, &mut it, &mut pools);
        let (a, _) = s.flush();
        assert_eq!(a, vec![(7, 0, 4)]);
        // 第二批：一模一样
        four_circuits(&mut s, &mut it, &mut pools);
        let (b, stale) = s.flush();
        assert_eq!(b, vec![(7, 0, 4)], "复用旧段 → 句柄不变");
        assert!(stale.is_empty());
        assert_eq!(s.len(), 4, "视图仍是 4 行（不是 8）");
        assert_eq!(s.entries().len(), 4, "arena 也没长");
        assert_eq!(s.counts().reused, 1);
        assert_eq!(s.live_spans().len(), 1);
    }

    #[test]
    fn changed_group_appends_a_new_span_but_the_view_shows_only_the_latest() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = N7Circuits::new();
        four_circuits(&mut s, &mut it, &mut pools);
        s.flush();
        // 第二批：少了一条（SC=611 没了）
        for i in 0..3u16 {
            add(
                &mut s,
                &mut it,
                &mut pools,
                &format!(
                    "MN=1000,SC={},EC={},SCIC=9,TG=7,TGN=\"T7\",TID={},CS=USE",
                    608 + i,
                    608 + i,
                    100 + i
                ),
            );
        }
        let (spans, _) = s.flush();
        assert_eq!(spans, vec![(7, 4, 3)], "内容变了 → 追加新段");
        assert_eq!(s.len(), 3, "视图只反映当前态");
        assert_eq!(s.entries().len(), 7, "arena 保留历史（4 旧 + 3 新）");
        assert_eq!(s.live_spans().len(), 1);
        assert_eq!(s.counts().reused, 0);
    }

    #[test]
    fn second_batch_reports_stale_tg_so_the_handle_can_be_zeroed() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = N7Circuits::new();
        add(&mut s, &mut it, &mut pools, "MN=1,SC=10,TG=5,TID=1,CS=USE");
        let (spans, _) = s.flush();
        assert_eq!(spans, vec![(5, 0, 1)]);
        // 第二批没有 TG=5 的电路 → 要把它 N7TG 行上的 circ_len 清 0，视图也不能再有它
        let (spans2, stale) = s.flush();
        assert!(spans2.is_empty());
        assert_eq!(stale, vec![5]);
        assert_eq!(s.len(), 0, "当前态里 TG=5 已经没了");
        assert_eq!(s.entries().len(), 1, "历史还在（as_of 要用）");
    }

    #[test]
    fn rmv_drops_the_circuit() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = N7Circuits::new();
        add(&mut s, &mut it, &mut pools, "MN=1,SC=10,TG=5,TID=1,CS=USE");
        add(&mut s, &mut it, &mut pools, "MN=1,SC=11,TG=5,TID=2,CS=USE");
        s.apply_line(Op::Rmv, b"MN=1,SC=10,TG=5", &mut it, &mut pools);
        let (spans, _) = s.flush();
        assert_eq!(spans, vec![(5, 0, 1)], "只剩 SC=11 一条");
        assert_eq!(s.col_u32(0, N7Circuits::col_index("SC")), Some(11));
    }

    #[test]
    fn second_cs_spelling_is_counted_not_silently_dropped() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = N7Circuits::new();
        add(&mut s, &mut it, &mut pools, "MN=1,SC=1,TG=5,CS=USE");
        add(&mut s, &mut it, &mut pools, "MN=1,SC=2,TG=5,CS=use");
        add(&mut s, &mut it, &mut pools, "MN=1,SC=3,TG=5,CS=USE");
        s.flush();
        assert_eq!(s.counts().cs_odd, 1, "同一位第二种词形要计数");
    }

    #[test]
    fn extra_param_variation_inside_a_group_is_counted() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut s = N7Circuits::new();
        add(&mut s, &mut it, &mut pools, "MN=1,SC=1,TG=5,MD=FRE");
        add(&mut s, &mut it, &mut pools, "MN=1,SC=2,TG=5,MD=OTH");
        s.flush();
        assert_eq!(s.counts().extra_odd, 1);
        assert_eq!(s.head(5).unwrap().md, b"FRE", "保留首见值");
        assert_eq!(s.lifted_stats().3, 1, "偏离数要进 lifted 台账");
    }
}
