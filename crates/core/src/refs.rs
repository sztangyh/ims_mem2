//! Ref 物化（P3/P4）：把"业务引用"从"每个 MOC 自己存一份字符串"变成**可按需解析的指针**。
//!
//! 口径（`../DESIGN.md` P3 / D37）：
//! - **NE 内**（跨网元引用天然不存在，D7 实测）：指针不需要带 `ne`；
//! - **先定义后引用**：整份文件导入完（`finish`）再建索引，单趟扫描就够；
//! - **`match` 分级消费**：只吃 `match=eq`（`tools/gen_refs.py` 已过滤）；
//! - **不物化 4 百万个指针**：只物化"目标键值 → 行号"的索引（大小 ≈ 目标表行数），
//!   源行的指针在查询时两步解析（源值 → 目标索引 → 行号）。
//!   实测：4,443,215 条引用如果逐条物化要 **+62.6 MB**，改成目标索引后只要几 MB。
//!
//! 悬空（目标行不存在）不丢信息：源列的原文仍在行里。

use std::collections::HashMap;

use crate::refs_gen::EDGES;
use crate::sym::Sym;

/// 一个网元的引用索引（按 `refs_gen::EDGES` 的下标对齐）。
#[derive(Debug, Default, Clone)]
pub struct NeRefs {
    /// `(目标表, 目标列)` → 值 → 行号（文本键）
    sym_idx: HashMap<(&'static str, u16), HashMap<Sym, u32>>,
    /// 同上（数值键）
    num_idx: HashMap<(&'static str, u16), HashMap<u32, u32>>,
    /// 同上（A2.5：宽值列，Ip/Eid/Bits 的 u64 形态）
    wide_idx: HashMap<(&'static str, u16), HashMap<u128, u32>>,
    /// 每个 edge 的命中/悬空/缺表计数（统计用，不占行级内存）
    pub resolved: Vec<u64>,
    pub dangling: Vec<u64>,
    pub missing_table: Vec<u64>,
}

impl NeRefs {
    pub fn new() -> Self {
        let n = EDGES.len();
        NeRefs {
            sym_idx: HashMap::new(),
            num_idx: HashMap::new(),
            wide_idx: HashMap::new(),
            resolved: vec![0; n],
            dangling: vec![0; n],
            missing_table: vec![0; n],
        }
    }

    pub(crate) fn put_sym(&mut self, tbl: &'static str, col: u16, m: HashMap<Sym, u32>) {
        self.sym_idx.insert((tbl, col), m);
    }

    pub(crate) fn put_num(&mut self, tbl: &'static str, col: u16, m: HashMap<u32, u32>) {
        self.num_idx.insert((tbl, col), m);
    }

    /// A2.5：宽值列索引（Ip/Eid/Bits）。
    pub(crate) fn put_wide(&mut self, tbl: &'static str, col: u16, m: HashMap<u128, u32>) {
        self.wide_idx.insert((tbl, col), m);
    }

    /// 文本值 → 目标行号。
    #[inline]
    pub fn lookup_sym(&self, edge: usize, v: Sym) -> Option<u32> {
        let e = &EDGES[edge];
        self.sym_idx.get(&(e.to, e.to_col)).and_then(|m| m.get(&v)).copied()
    }

    /// 数值 → 目标行号。
    #[inline]
    pub fn lookup_num(&self, edge: usize, v: u32) -> Option<u32> {
        let e = &EDGES[edge];
        self.num_idx.get(&(e.to, e.to_col)).and_then(|m| m.get(&v)).copied()
    }

    /// A2.5：宽值 → 目标行号。
    #[inline]
    pub fn lookup_wide(&self, edge: usize, v: u128) -> Option<u32> {
        let e = &EDGES[edge];
        self.wide_idx.get(&(e.to, e.to_col)).and_then(|m| m.get(&v)).copied()
    }

    /// `(边数, 命中总数, 悬空总数, 缺表总数)`
    pub fn totals(&self) -> (usize, u64, u64, u64) {
        (
            EDGES.len(),
            self.resolved.iter().sum(),
            self.dangling.iter().sum(),
            self.missing_table.iter().sum(),
        )
    }

    /// 有命中的边数。
    pub fn hit_edges(&self) -> usize {
        self.resolved.iter().filter(|&&n| n > 0).count()
    }

    /// 索引里一共装了多少条 `值 → 行号`（用来对比"逐条物化"的成本）。
    pub fn index_entries(&self) -> usize {
        self.sym_idx.values().map(|m| m.len()).sum::<usize>()
            + self.num_idx.values().map(|m| m.len()).sum::<usize>()
            + self.wide_idx.values().map(|m| m.len()).sum::<usize>()
    }
}
