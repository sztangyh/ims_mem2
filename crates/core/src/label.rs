//! `(MOC, PARAM)` 标签池：1 字节枚举（`Option<Label>` 靠 NonZero niche 也是 1 B）。
//!
//! 语义沿用 ims_mem：
//! - 每个 `(MOC, PARAM)` 一个独立池（同一个 MOC 里 `NETINFO` 与 `SGN` 互不干扰）；
//! - 池挂在 `Library` 上，**跨 NE 共享**；
//! - 池上限 255；满了视为"非枚举列"，返回 `None`（调用方记 `fallbacks`，值不进行）。
//!
//! 为什么不用 Rust enum：值的全集不在编译期（首现即入池，设备会加私有词），
//! 且标签维度是 `(MOC, PARAM)` 而不是 MOC。

use std::collections::HashMap;
use std::num::NonZeroU8;

use crate::sym::Sym;

/// 池内下标（0-based，存成 1-based 的 NonZero 以便 niche 优化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Label(NonZeroU8);

impl Label {
    #[inline]
    pub fn as_u8(self) -> u8 {
        self.0.get() - 1
    }

    /// 由 0-based 下标重建（版本链/桥接解包用）。
    #[inline]
    pub fn from_u8_0(i: u8) -> Option<Label> {
        NonZeroU8::new(i.checked_add(1)?).map(Label)
    }
}

#[derive(Debug, Clone)]
pub struct Pool {
    pub name: &'static str,
    vals: Vec<Sym>,
    index: HashMap<Sym, u8>,
}

#[derive(Debug, Default)]
pub struct LabelCols {
    pools: Vec<Pool>,
}

/// 池上限：`u8` 去掉 0（NonZero）。
pub const POOL_MAX: usize = 255;

impl LabelCols {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册列（幂等），返回 pool_id。列名形如 `"ASBR.NETINFO"`。
    pub fn register(&mut self, name: &'static str) -> usize {
        if let Some(i) = self.pools.iter().position(|p| p.name == name) {
            return i;
        }
        self.pools.push(Pool {
            name,
            vals: Vec::new(),
            index: HashMap::new(),
        });
        self.pools.len() - 1
    }

    /// 查或建标签；池满返回 `None`（调用方走 fallbacks 路径）。
    pub fn label_of(&mut self, pool: usize, v: Sym) -> Option<Label> {
        let p = &mut self.pools[pool];
        if let Some(&i) = p.index.get(&v) {
            return Some(Label(NonZeroU8::new(i + 1)?));
        }
        if p.vals.len() >= POOL_MAX {
            return None;
        }
        let i = p.vals.len() as u8;
        p.vals.push(v);
        p.index.insert(v, i);
        NonZeroU8::new(i + 1).map(Label)
    }

    /// 按名查 pool id（**不注册**；导出期用，避免 `&mut`）。
    pub fn find(&self, name: &str) -> Option<usize> {
        self.pools.iter().position(|p| p.name == name)
    }

    /// 标签 → 词（驻留符号）。池号越界返回 `None`（导出代码可用 `usize::MAX` 表示"没有这个池"）。
    pub fn label_sym(&self, pool: usize, l: Label) -> Option<Sym> {
        self.pools
            .get(pool)
            .and_then(|p| p.vals.get(l.as_u8() as usize))
            .copied()
    }

    #[inline]
    pub fn pool_len(&self) -> usize {
        self.pools.len()
    }

    /// 报告用：每列 distinct 值数（一眼看出有没有逼近 255 的池）。
    pub fn pool_sizes(&self) -> Vec<(&'static str, usize)> {
        self.pools.iter().map(|p| (p.name, p.vals.len())).collect()
    }

    /// 粗估占用（Vec + HashMap 开销，报告用）。
    pub fn bytes(&self) -> usize {
        self.pools
            .iter()
            .map(|p| p.vals.len() * 4 + p.vals.capacity() * 4 + p.index.capacity() * 9)
            .sum::<usize>()
            + self.pools.len() * std::mem::size_of::<Pool>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sym::Interner;

    #[test]
    fn label_is_one_byte() {
        assert_eq!(std::mem::size_of::<Label>(), 1);
        assert_eq!(std::mem::size_of::<Option<Label>>(), 1);
    }

    #[test]
    fn pools_are_independent() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let p0 = pools.register("ASBR.NETINFO");
        let p1 = pools.register("ASBR.SGN");
        let p0b = pools.register("ASBR.NETINFO"); // 幂等
        assert_eq!(p0, p0b);

        let sz = it.intern(b"SZ");
        let dg = it.intern(b"DG");
        let a = pools.label_of(p0, sz).unwrap();
        let b = pools.label_of(p0, dg).unwrap();
        assert_ne!(a, b);
        assert_eq!(pools.label_of(p0, sz).unwrap(), a, "同值返回同标签");
        assert_eq!(pools.label_of(p1, sz).unwrap(), a, "跨池独立编号，可能相同");
        assert_eq!(pools.label_sym(p0, a).map(|s| it.resolve(s)), Some(&b"SZ"[..]));
        assert_eq!(pools.pool_sizes(), vec![("ASBR.NETINFO", 2), ("ASBR.SGN", 1)]);
    }

    #[test]
    fn pool_overflow_returns_none() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let p = pools.register("X.Y");
        for i in 0..POOL_MAX {
            let s = it.intern(format!("V{i}").as_bytes());
            assert!(pools.label_of(p, s).is_some(), "第 {i} 个应仍在池内");
        }
        let extra = it.intern(b"OVERFLOW");
        assert_eq!(pools.label_of(p, extra), None, "池满必须返回 None，由调用方记 fallbacks");
        assert_eq!(pools.pool_sizes()[0].1, POOL_MAX);
    }
}
