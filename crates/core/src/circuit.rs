//! 电路明细的两种无损表示（P7 / D53）。
//!
//! 依据（6 份实测数据）：
//! - **PRA 是随路信令**：信令与话路同 PCM，每个 PCM 恰好 **32 时隙**，
//!   实测每组电路条数全是 32 的整数倍（32/64/96/128），`SC == EC`、`CS ∈ {USE, UNU}`
//!   → 可以无损压成"每 PCM 一个 `u32` 位图"（10 B / 32 条）。
//! - **N7 是公共信令**：信令通道可与话路分离，`SC` 稀疏、每 TG 32~672 条、
//!   还有 `SCF`/`SCIC` → 不能用位图，用稀疏条目（11 B / 条）。
//!
//! **不丢信息是硬约束**（D52）：块化只在"完全符合 PCM 形态"时使用，
//! 不符合的组**不静默丢弃**，而是计入 `exceptions` 并回退成稀疏条目。
//!
//! # 谁在用（2026-11 状态）
//!
//! - **PRA 已落地为"块即行"**（D54）：`schema::pra_gen` 用 [`PcmBlock`] 的字段语义
//!   （`mn/sc0/tid0/use_mask`）把 4,376 个 PCM 块直接做成表行，**不走** arena。
//! - [`CircuitArena`] / [`BlockBuild`] / [`Circuit`] 仍是 **N7 的零件**：按 D55 的实测
//!   （N7TKC 稀疏化只省 ≈0.2 MB）**记账不接线**，先留在这里 + 单测锁住语义。

/// 一个 PCM 块：32 个时隙的占用位图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmBlock {
    pub mn: u16,
    /// 起始时隙（通常 0/32/64/96）
    pub sc0: u16,
    /// 起始电路号（`tid` 与 `sc` 同差线性，实测 2,899/3,461 满块）
    pub tid0: u16,
    /// bit i = `sc0 + i` 这个时隙是 USE（1）还是 UNU（0）
    pub use_mask: u32,
}

impl PcmBlock {
    /// 这个块里的电路条数（实测恒为 32）。
    pub const SLOTS: u16 = 32;

    #[inline]
    pub fn is_use(&self, i: u16) -> bool {
        i < Self::SLOTS && (self.use_mask >> i) & 1 == 1
    }

    #[inline]
    pub fn use_count(&self) -> u32 {
        self.use_mask.count_ones()
    }

    /// 还原成扁平行：`(mn, sc, tid, use)`。
    pub fn expand(&self) -> impl Iterator<Item = (u16, u16, u16, bool)> + '_ {
        (0..Self::SLOTS).map(move |i| {
            (
                self.mn,
                self.sc0 + i,
                self.tid0 + i,
                (self.use_mask >> i) & 1 == 1,
            )
        })
    }
}

/// 稀疏电路条目（N7 用；PRA 的例外回退也用它）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Circuit {
    pub mn: u16,
    pub sc: u16,
    pub ec: u16,
    pub tid: u16,
    pub scic: u16,
    /// bit0 = CS(USE)；bit1 = SCF
    pub flags: u8,
}

impl Circuit {
    pub const F_USE: u8 = 1 << 0;
    pub const F_SCF: u8 = 1 << 1;

    #[inline]
    pub fn is_use(&self) -> bool {
        self.flags & Self::F_USE != 0
    }

    #[inline]
    pub fn is_scf(&self) -> bool {
        self.flags & Self::F_SCF != 0
    }
}

/// 一块命中集合的构建结果：块 + 未能块化的例外。
#[derive(Debug, Default, Clone)]
pub struct BlockBuild {
    pub blocks: Vec<PcmBlock>,
    /// 形态不符（非 32 连续 / CS 非 USE|UNU / tid 非线性）的组 → 回退成稀疏条目
    pub fallback: Vec<Circuit>,
    pub exceptions: u64,
}

impl BlockBuild {
    /// 尝试把一组 `(sc, tid, use)` 压成若干 32 连续块；不符就整组回退成稀疏条目。
    ///
    /// `sc` 会按升序处理；`tid` 必须与 `sc` 同差（`tid - sc` 恒定），否则整组回退。
    pub fn push_group(&mut self, mn: u16, group: &mut [(u16, u16, bool)]) {
        if group.is_empty() {
            return;
        }
        group.sort_unstable_by_key(|x| x.0);
        // 形态检查：条数必须是 32 的倍数、SC 连续、tid 同差
        let ok_count = group.len().is_multiple_of(32);
        let sc0 = group[0].0;
        let contiguous = group
            .iter()
            .enumerate()
            .all(|(i, x)| x.0 == sc0 + i as u16);
        let tid_base = group[0].1 as i32 - group[0].0 as i32;
        let tid_linear = group
            .iter()
            .all(|x| x.1 as i32 - x.0 as i32 == tid_base);
        if !(ok_count && contiguous && tid_linear) {
            self.exceptions += 1;
            for (sc, tid, u) in group.iter() {
                self.fallback.push(Circuit {
                    mn,
                    sc: *sc,
                    ec: *sc,
                    tid: *tid,
                    scic: 0,
                    flags: if *u { Circuit::F_USE } else { 0 },
                });
            }
            return;
        }
        for chunk in group.chunks(32) {
            let mut mask = 0u32;
            for (i, (_, _, u)) in chunk.iter().enumerate() {
                if *u {
                    mask |= 1 << i;
                }
            }
            self.blocks.push(PcmBlock {
                mn,
                sc0: chunk[0].0,
                tid0: chunk[0].1,
                use_mask: mask,
            });
        }
    }

    /// 展开后的电路总条数（对账用：必须等于输入条数）。
    pub fn expanded_len(&self) -> usize {
        self.blocks.len() * PcmBlock::SLOTS as usize + self.fallback.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(n: u16) -> Vec<(u16, u16, bool)> {
        (0..n)
            .map(|i| (i, 100 + i, i % 3 != 0)) // tid 与 sc 同差
            .collect()
    }

    #[test]
    fn one_pcm_becomes_one_block_and_expands_back() {
        let mut b = BlockBuild::default();
        let mut g = group(32);
        let src = g.clone();
        b.push_group(7, &mut g);
        assert_eq!(b.blocks.len(), 1);
        assert_eq!(b.exceptions, 0);
        let blk = b.blocks[0];
        assert_eq!((blk.mn, blk.sc0, blk.tid0), (7, 0, 100));
        // 展开必须逐条还原（无损）
        let back: Vec<(u16, u16, u16, bool)> = blk.expand().collect();
        assert_eq!(back.len(), 32);
        for (i, (sc, tid, u)) in src.iter().enumerate() {
            assert_eq!(back[i], (7, *sc, *tid, *u));
        }
    }

    #[test]
    fn two_pcms_become_two_blocks() {
        let mut b = BlockBuild::default();
        let mut g = group(64);
        b.push_group(7, &mut g);
        assert_eq!(b.blocks.len(), 2, "64 条 = 2 个 PCM");
        assert_eq!(b.blocks[1].sc0, 32);
        assert_eq!(b.expanded_len(), 64);
    }

    #[test]
    fn irregular_group_falls_back_without_losing_data() {
        let mut b = BlockBuild::default();
        // 不连续 + 非 32 倍数 → 整组回退，不丢
        let mut g = vec![(0u16, 10u16, true), (2, 12, false), (5, 15, true)];
        b.push_group(9, &mut g);
        assert_eq!(b.blocks.len(), 0);
        assert_eq!(b.exceptions, 1);
        assert_eq!(b.fallback.len(), 3);
        assert!(b.fallback[0].is_use());
        assert!(!b.fallback[1].is_use());
        assert_eq!(b.expanded_len(), 3, "回退后条数不变");
    }

    #[test]
    fn mask_counts_use_slots() {
        let mut b = BlockBuild::default();
        let mut g = group(32);
        b.push_group(1, &mut g);
        let blk = b.blocks[0];
        let expect = (0..32u16).filter(|i| i % 3 != 0).count() as u32;
        assert_eq!(blk.use_count(), expect);
        assert!(blk.is_use(1) && !blk.is_use(0) && !blk.is_use(33));
    }
}

/// 每 NE 的电路 arena：PRA 的 PCM 块与 N7 的稀疏条目分开存，
/// 行内只放 `(start, len)` 区间句柄。
///
/// 为什么两个 arena 而不是一个联合体：两种形态的**元素大小与语义不同**
/// （10 B 的块 vs 11 B 的条目），分开存可以让 PRA 只付块的开销、
/// 也让"展开成扁平行"的两条路径各自独立、互不影响。
#[derive(Debug, Default, Clone)]
pub struct CircuitArena {
    blocks: Vec<PcmBlock>,
    sparse: Vec<Circuit>,
}

impl CircuitArena {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一个 PCM 块，返回其下标（行内 `block_start` 用它）。
    pub fn push_block(&mut self, b: PcmBlock) -> u32 {
        let i = self.blocks.len() as u32;
        self.blocks.push(b);
        i
    }

    /// 追加一条稀疏电路，返回其下标。
    pub fn push_sparse(&mut self, c: Circuit) -> u32 {
        let i = self.sparse.len() as u32;
        self.sparse.push(c);
        i
    }

    /// 取一段 PCM 块（行内 `(block_start, block_len)`）。
    pub fn block_span(&self, start: u32, len: u16) -> &[PcmBlock] {
        let s = start as usize;
        let e = (s + len as usize).min(self.blocks.len());
        if s >= self.blocks.len() {
            return &[];
        }
        &self.blocks[s..e]
    }

    /// 取一段稀疏电路。
    pub fn sparse_span(&self, start: u32, len: u16) -> &[Circuit] {
        let s = start as usize;
        let e = (s + len as usize).min(self.sparse.len());
        if s >= self.sparse.len() {
            return &[];
        }
        &self.sparse[s..e]
    }

    pub fn blocks(&self) -> &[PcmBlock] {
        &self.blocks
    }

    pub fn sparse(&self) -> &[Circuit] {
        &self.sparse
    }

    /// 展开后的电路总条数（对账用）。
    pub fn expanded_len(&self) -> usize {
        self.blocks.len() * PcmBlock::SLOTS as usize + self.sparse.len()
    }

    /// arena 自身占用（报告用）。
    pub fn bytes(&self) -> usize {
        self.blocks.len() * std::mem::size_of::<PcmBlock>()
            + self.sparse.len() * std::mem::size_of::<Circuit>()
    }
}

#[cfg(test)]
mod arena_tests {
    use super::*;

    #[test]
    fn arena_keeps_spans_and_counts() {
        let mut a = CircuitArena::new();
        let i0 = a.push_block(PcmBlock { mn: 1, sc0: 0, tid0: 10, use_mask: 0xFFFF_FFFF });
        let i1 = a.push_block(PcmBlock { mn: 1, sc0: 32, tid0: 42, use_mask: 0 });
        assert_eq!((i0, i1), (0, 1));
        a.push_sparse(Circuit { mn: 2, sc: 608, ec: 608, tid: 8192, scic: 7, flags: Circuit::F_USE });
        a.push_sparse(Circuit { mn: 2, sc: 610, ec: 610, tid: 8194, scic: 8, flags: 0 });

        let span = a.block_span(0, 2);
        assert_eq!(span.len(), 2);
        assert_eq!(span[1].sc0, 32);
        // 越界安全
        assert_eq!(a.block_span(5, 3).len(), 0);
        assert_eq!(a.block_span(1, 99).len(), 1);

        assert_eq!(a.expanded_len(), 2 * 32 + 2, "两个满块 + 两条稀疏");
        assert_eq!(a.sparse_span(0, 2).len(), 2);
        assert_eq!(a.sparse_span(0, 2)[1].scic, 8);
        assert!(a.bytes() > 0);
    }
}
