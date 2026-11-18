//! SRTCOST 的"成本方案"归一化（P7 / D50）。
//!
//! 业务语义：对某个 `(lp, pfx)`，配置**若干条** `(子路由 sr, 分流百分比 psr, 出局成本 cst)`，
//! 呼叫按 `psr` 的比例分到不同子路由上出局。
//!
//! 实测（6 文件）：166,638 条 SRTCOST 行 → 只有 **29,274 个不同的 `(lp,pfx)`**，
//! 而这些前缀共用 **29 种成本方案**。所以行只需要 `(lp, pfx, scheme_idx)`，
//! 方案进"内容寻址"的池子：**同内容复用同一个 idx，idx 永不复用**。
//!
//! `idx 永不复用` 是 SCD2 能成立的前提：版本链里存的是 `scheme_idx`，
//! 旧 idx 永远指向它当年的内容，`as_of` 回放只是查表，不需要给方案也做版本链。

use std::collections::HashMap;

/// 成本方案里的一条：`(子路由号, 分流百分比, 成本)`。
pub type CostEntry = (u16, u8, u16);

/// 一个成本方案：按 `sr` 升序的条目列表（去重、同 `sr` 后写覆盖前写）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SrtScheme {
    pub entries: Vec<CostEntry>,
}

impl SrtScheme {
    /// 由扁平条目构造：同 `sr` 保留**最后一次**（与"后写覆盖"一致），再按 `sr` 排序。
    pub fn from_flat(flat: Vec<CostEntry>) -> Self {
        let mut m: HashMap<u16, CostEntry> = HashMap::with_capacity(flat.len());
        for e in flat {
            m.insert(e.0, e);
        }
        let mut entries: Vec<CostEntry> = m.into_values().collect();
        entries.sort_unstable();
        SrtScheme { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 内容寻址的方案池（每 NE 一份）。
#[derive(Debug, Default, Clone)]
pub struct SchemeStore {
    vals: Vec<SrtScheme>,
    index: HashMap<SrtScheme, u32>,
    /// 条目总数（= 展开后的扁平行数，用来和 ims_mem 的 table_report 对账）
    entries: u64,
}

impl SchemeStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 驻留一个方案，返回下标（内容相同复用）。
    pub fn intern(&mut self, s: SrtScheme) -> u32 {
        if let Some(&i) = self.index.get(&s) {
            return i;
        }
        let i = self.vals.len() as u32;
        self.entries += s.entries.len() as u64;
        self.index.insert(s.clone(), i);
        self.vals.push(s);
        i
    }

    #[inline]
    pub fn get(&self, idx: u32) -> Option<&SrtScheme> {
        self.vals.get(idx as usize)
    }

    /// 方案个数。
    pub fn len(&self) -> usize {
        self.vals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vals.is_empty()
    }

    /// 展开后的总条目数（用于对账：应等于 SRTCOST 的原始行数）。
    pub fn entries(&self) -> u64 {
        self.entries
    }

    /// 池子自身占用（报告用）。
    pub fn bytes(&self) -> usize {
        self.vals
            .iter()
            .map(|s| s.entries.len() * std::mem::size_of::<CostEntry>() + std::mem::size_of::<SrtScheme>())
            .sum()
    }
}


/// 批内聚合器：扁平 `SET/ADD/RMV` 行 → 每个 `(lp, pfx)` 一个方案。
///
/// 键用**字冠原文**（`K'0086…`，保留前导零）而不是 `PfxNum`：
/// 落盘时要合成一条 `LP=..,PFX=K'..,SCHEME=idx` 的 MML 体，原文必须能还原。
#[derive(Debug, Default, Clone)]
pub struct SrtCostPending {
    /// `(lp, pfx 原文) → 条目列表`（同 `sr` 后写覆盖交给 `SrtScheme::from_flat`）
    pairs: std::collections::HashMap<(u16, Vec<u8>), Vec<CostEntry>>,
    /// 见过的条目数（对账：应等于原始 SRTCOST 行数）
    seen: u64,
}

impl SrtCostPending {
    pub fn new() -> Self {
        Self::default()
    }

    /// 收一条扁平行。`op_rmv` 时删掉该 `sr`。
    pub fn push(&mut self, lp: u16, pfx_text: &[u8], sr: u16, psr: u8, cst: u16, op_rmv: bool) {
        let e = self.pairs.entry((lp, pfx_text.to_vec())).or_default();
        if op_rmv {
            e.retain(|x| x.0 != sr);
        } else {
            e.push((sr, psr, cst));
            self.seen += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// 本批涉及的 `(lp, pfx)` 个数。
    pub fn pairs(&self) -> usize {
        self.pairs.len()
    }

    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// 清空并返回 `(lp, pfx 原文, 方案 idx)`；输出按 `(lp, pfx)` 升序，便于复现。
    pub fn flush(&mut self, store: &mut SchemeStore) -> Vec<(u16, Vec<u8>, u32)> {
        let mut out: Vec<(u16, Vec<u8>, u32)> = std::mem::take(&mut self.pairs)
            .into_iter()
            .map(|((lp, pfx), flat)| {
                let idx = store.intern(SrtScheme::from_flat(flat));
                (lp, pfx, idx)
            })
            .collect();
        out.sort_unstable();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_by_content_and_keeps_idx_stable() {
        let mut st = SchemeStore::new();
        let a = SrtScheme::from_flat(vec![(1, 50, 10), (2, 50, 20)]);
        let b = SrtScheme::from_flat(vec![(1, 50, 10), (2, 50, 20)]); // 同内容
        let c = SrtScheme::from_flat(vec![(1, 100, 10)]);
        let ia = st.intern(a.clone());
        let ib = st.intern(b);
        let ic = st.intern(c);
        assert_eq!(ia, ib, "同内容必须复用同一个 idx");
        assert_ne!(ia, ic);
        assert_eq!(st.len(), 2, "只有两种方案");
        assert_eq!(st.entries(), 3, "条目总数 = 2 + 1");
        // 旧 idx 永远指向它当年的内容（SCD2 依赖这一点）
        assert_eq!(st.get(ia).unwrap(), &a);
    }

    #[test]
    fn last_write_wins_per_subroute() {
        // 同一个 sr 出现两次 → 后写覆盖；与扁平表的 SET 语义一致
        let s = SrtScheme::from_flat(vec![(1, 50, 10), (1, 80, 99), (2, 50, 20)]);
        assert_eq!(s.entries, vec![(1, 80, 99), (2, 50, 20)]);
    }

    #[test]
    fn pending_groups_flat_rows_into_schemes() {
        let mut p = SrtCostPending::new();
        // 两个 (lp,pfx)，各 2~3 条；其中一个 sr 重复（后写覆盖）
        p.push(0, b"K'0086", 1, 50, 10, false);
        p.push(0, b"K'0086", 2, 50, 20, false);
        p.push(0, b"K'0086", 2, 70, 30, false);
        p.push(0, b"K'0087", 1, 50, 10, false);
        assert_eq!(p.pairs(), 2);
        assert_eq!(p.seen(), 4);

        let mut st = SchemeStore::new();
        let rows = p.flush(&mut st);
        assert_eq!(rows.len(), 2, "两个 (lp,pfx) → 两行");
        // K'0087 的方案 = K'0086 的子集但不同 → 两个不同方案
        assert_eq!(st.len(), 2);
        // 条目总数 = 展开后的扁平行数（对账口径）
        assert_eq!(st.entries(), 3, "K'0086 两条（sr2 覆盖）+ K'0087 一条");
        assert!(p.is_empty(), "flush 之后清空");
    }

    #[test]
    fn pending_rmv_drops_the_entry() {
        let mut p = SrtCostPending::new();
        p.push(0, b"K'1", 1, 50, 10, false);
        p.push(0, b"K'1", 2, 50, 20, false);
        p.push(0, b"K'1", 2, 0, 0, true); // RMV sr=2
        let mut st = SchemeStore::new();
        p.flush(&mut st);
        assert_eq!(st.get(0).unwrap().entries, vec![(1, 50, 10)]);
    }

    #[test]
    fn empty_scheme_is_valid() {
        let mut st = SchemeStore::new();
        let i = st.intern(SrtScheme::from_flat(vec![]));
        assert!(st.get(i).unwrap().is_empty());
    }
}
