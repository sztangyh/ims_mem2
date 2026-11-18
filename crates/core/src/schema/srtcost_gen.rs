//! `SRTCOST` 的宏声明版（P0.4）：第一个带 **Pfx 复合键** 的真实表。
//!
//! 口径（与 ims_mem 的 `SRTCOST` 一致）：
//! - 键 `[cst, lp, pfx, psr, sr]`，其中 `pfx` 是 `K'...` 字冠；
//! - `srn` 只是名称列（不是键）—— 引用漂移由导出对账发现，不分裂实体；
//! - 该表在 6 网元里是 **多行 SET**（不是 ADD），所以 `KEYED = true` 但走 `Op::Set` 路径
//!   （命中即合并，未命中即插入）。
//!
//! ⚠ `Pfx` 的行内表示是**原文 `Sym`**，数字形态 `PfxNum` 只出现在 `Pending`（用于键与比较）。
//! 这是 `DESIGN.md` 里"Pfx 方案 A"的落地；历史查询拿到的行只有原文，数字按需重解析。

use crate::frombytes::unquote;
use crate::mml_table;
use crate::scan::Params;
use crate::sym::Interner;

mml_table! {
    SRTCOST, Srtcost,
    key [lp, pfx],
    lp: U16,
    pfx: Pfx,
    scheme: U32,
}

/// SRTCOST 的归一化外壳（P7 / D50）：扁平行按 `(lp,pfx)` 聚合成"成本方案"，
/// 批末把 `scheme_idx` 落到一条 `(lp,pfx)` 行上。
#[derive(Default)]
pub struct SrtcostShim {
    pub core: SrtcostTable,
    pending: crate::srtcost_scheme::SrtCostPending,
    schemes: crate::srtcost_scheme::SchemeStore,
}

impl std::ops::Deref for SrtcostShim {
    type Target = SrtcostTable;
    fn deref(&self) -> &SrtcostTable {
        &self.core
    }
}
impl std::ops::DerefMut for SrtcostShim {
    fn deref_mut(&mut self) -> &mut SrtcostTable {
        &mut self.core
    }
}

impl SrtcostShim {
    pub fn new(batch: i64) -> Self {
        let mut core = SrtcostTable::new();
        core.set_batch(batch);
        SrtcostShim {
            core,
            pending: crate::srtcost_scheme::SrtCostPending::new(),
            schemes: crate::srtcost_scheme::SchemeStore::new(),
        }
    }

    pub fn schemes(&self) -> &crate::srtcost_scheme::SchemeStore {
        &self.schemes
    }

    /// 展开后的扁平行数（对账用：应等于原始 SRTCOST 行数）。
    pub fn flat_rows(&self) -> usize {
        (0..self.core.len() as u32)
            .filter(|i| self.core.is_alive(*i))
            .filter_map(|i| self.core.row(i))
            .filter_map(|r| r.scheme())
            .filter_map(|idx| self.schemes.get(idx))
            .map(|s| s.len())
            .sum()
    }

    /// 收一条扁平行：`CST/LP/PFX/PSR/SR`（`SRN` 不需要，等价于 `SR`）。
    pub fn apply(
        &mut self,
        op: crate::scan::Op,
        body: &[u8],
        it: &mut Interner,
        pools: &mut crate::label::LabelCols,
    ) {
        let mut lp = None;
        let mut pfx = None;
        let mut psr = None;
        let mut sr = None;
        let mut cst = None;
        for (k, v) in Params::new(body) {
            if k.eq_ignore_ascii_case(b"LP") {
                lp = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"PFX") {
                pfx = Some(unquote(v));
            } else if k.eq_ignore_ascii_case(b"PSR") {
                psr = crate::frombytes::parse_u8(v);
            } else if k.eq_ignore_ascii_case(b"SR") {
                sr = crate::frombytes::parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"CST") {
                cst = crate::frombytes::parse_u16(v);
            }
        }
        let (Some(lp), Some(pfx)) = (lp, pfx) else { return };
        let rmv = op == crate::scan::Op::Rmv;
        self.pending.push(
            lp,
            pfx,
            sr.unwrap_or(0),
            psr.unwrap_or(0),
            cst.unwrap_or(0),
            rmv,
        );
        let _ = (it, pools);
    }

    /// 批末：把方案落成 `(lp, pfx, scheme)` 行（合成一条 MML 体走现有 `apply(Set)`）。
    fn flush(&mut self, it: &mut Interner, pools: &mut crate::label::LabelCols) {
        if self.pending.is_empty() {
            return;
        }
        for (lp, pfx, idx) in self.pending.flush(&mut self.schemes) {
            let mut body = format!("LP={lp},PFX=");
            body.push_str(&String::from_utf8_lossy(&pfx));
            body.push_str(&format!(",SCHEME={idx}"));
            self.core
                .apply(crate::scan::Op::Set, body.as_bytes(), it, pools);
        }
    }

    pub fn set_batch(
        &mut self,
        batch: i64,
        it: &mut Interner,
        pools: &mut crate::label::LabelCols,
    ) {
        self.flush(it, pools);
        self.core.set_batch(batch);
    }

    pub fn finish(&mut self, it: &mut Interner, pools: &mut crate::label::LabelCols) {
        self.flush(it, pools);
        self.core.finish();
    }
}

#[cfg(test)]
mod tests {
    use crate::sym::Interner;
    use crate::{scan::Op, LabelCols};

    fn body(pfx: &str, sr: u16, psr: u8, cst: u16) -> Vec<u8> {
        format!("CST={cst},LP=15,PFX={pfx},PSR={psr},SR={sr},SRN=\"x\"").into_bytes()
    }

    #[test]
    fn flat_rows_collapse_into_one_row_per_pfx() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::SrtcostShim::new(100);
        // 同一 (lp,pfx) 的 3 条子路由 → 1 行
        t.apply(Op::Set, &body("K'0086755", 10, 50, 1), &mut it, &mut pools);
        t.apply(Op::Set, &body("K'0086755", 11, 50, 2), &mut it, &mut pools);
        t.apply(Op::Set, &body("K'0086755", 12, 0, 3), &mut it, &mut pools);
        // 另一个前缀
        t.apply(Op::Set, &body("K'0086756", 10, 100, 1), &mut it, &mut pools);
        assert_eq!(t.core.len(), 0, "flush 之前不建行");
        t.finish(&mut it, &mut pools);
        assert_eq!(t.core.len(), 2, "两个前缀 → 两行");
        assert_eq!(t.flat_rows(), 4, "展开后仍是 4 条扁平行");
        assert_eq!(t.schemes().len(), 2, "两种成本方案");
    }

    #[test]
    fn set_twice_same_prefix_keeps_latest_scheme() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::SrtcostShim::new(100);
        t.apply(Op::Set, &body("K'1", 10, 50, 1), &mut it, &mut pools);
        t.apply(Op::Set, &body("K'1", 11, 50, 2), &mut it, &mut pools);
        t.set_batch(200, &mut it, &mut pools);
        // 第二批：只剩 sr=10，且 psr 变了
        t.apply(Op::Set, &body("K'1", 10, 80, 1), &mut it, &mut pools);
        t.finish(&mut it, &mut pools);
        assert_eq!(t.core.len(), 1);
        assert_eq!(t.flat_rows(), 1, "第二批只有一条条目");
        assert_eq!(t.schemes().len(), 2, "两种方案：旧的两条 / 新的一条");
        assert_eq!(t.core.vhist().len(), 1, "方案变了 → 一条版本");
    }
}
