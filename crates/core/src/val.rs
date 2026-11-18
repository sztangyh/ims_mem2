//! 值表示：三态 `Val<T>` + 位打包 `Val32`。
//!
//! `Val<T>` 是 ims_mem 已验证的三态语义（缺参 / 已设 / 解析失败保原文），本 crate 沿用：
//! - `Default`：行内未出现 ⇒ ADD 时回退默认、MOD 时不改动；
//! - `Set(T)`：出现且解析成功；
//! - `Raw(Sym)`：出现但解析失败/超界 —— 驻留原文兜底，**绝不丢行**。
//!
//! `Val32` 是 P1 的位打包形态：`2 bit tag + 30 bit payload`，把 `u8/u16/bool/Label` 这类
//! 小值域字段从 8 B 压到 4 B（ims_mem README 的"已知限制 2"）。
//! ⚠ 并非所有类型都能打包：`PfxNum` 是 68 bit、`Uri` 需要 Sym + 号段，必须留 8 B。

use crate::sym::Sym;

/// 2 bit tag 放在高 2 位。
const TAG_SHIFT: u32 = 30;
const PAYLOAD_BITS: u32 = 30;
const PAYLOAD_MASK: u32 = (1 << PAYLOAD_BITS) - 1;

/// 字段三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Val<T> {
    #[default]
    Default,
    Set(T),
    Raw(Sym),
}

impl<T: Copy> Val<T> {
    #[inline]
    pub fn is_set(&self) -> bool {
        matches!(self, Val::Set(_))
    }

    #[inline]
    pub fn is_default(&self) -> bool {
        matches!(self, Val::Default)
    }

    /// 取值：`Set` 直接给，其余给 `default`（`Raw` 的原文要用 [`Val::raw`] 单独取）。
    #[inline]
    pub fn or(self, default: T) -> T {
        match self {
            Val::Set(t) => t,
            _ => default,
        }
    }

    #[inline]
    pub fn set(&self) -> Option<T> {
        match self {
            Val::Set(t) => Some(*t),
            _ => None,
        }
    }

    #[inline]
    pub fn raw(&self) -> Option<Sym> {
        match self {
            Val::Raw(s) => Some(*s),
            _ => None,
        }
    }
}

/// 可位打包的值类型（≤ 30 bit）。
pub trait PackedVal: Copy {
    const BITS: u32;
    fn to_u32(self) -> u32;
    fn from_u32(v: u32) -> Self;
}

impl PackedVal for u8 {
    const BITS: u32 = 8;
    #[inline]
    fn to_u32(self) -> u32 {
        self as u32
    }
    #[inline]
    fn from_u32(v: u32) -> Self {
        v as u8
    }
}

impl PackedVal for u16 {
    const BITS: u32 = 16;
    #[inline]
    fn to_u32(self) -> u32 {
        self as u32
    }
    #[inline]
    fn from_u32(v: u32) -> Self {
        v as u16
    }
}

impl PackedVal for bool {
    const BITS: u32 = 1;
    #[inline]
    fn to_u32(self) -> u32 {
        self as u32
    }
    #[inline]
    fn from_u32(v: u32) -> Self {
        v != 0
    }
}

/// 4 字节打包值：`2 bit tag | 30 bit payload`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Val32(u32);

impl Val32 {
    pub const DEFAULT: Val32 = Val32(0);

    /// 30 bit payload 能装下的最大无符号值（公开给宏做溢出判定）。
    pub const MAX: u32 = PAYLOAD_MASK;

    #[inline]
    pub fn set(v: u32) -> Option<Val32> {
        if v <= PAYLOAD_MASK {
            Some(Val32((1 << TAG_SHIFT) | v))
        } else {
            None
        }
    }

    #[inline]
    pub fn set_masked(v: u32) -> Val32 {
        debug_assert!(v <= PAYLOAD_MASK, "值超出 30 bit，应退回 8 B 的 Val<T>");
        Val32((1 << TAG_SHIFT) | (v & PAYLOAD_MASK))
    }

    #[inline]
    pub fn raw(s: Sym) -> Option<Val32> {
        let i = s.idx() as u32;
        if i <= PAYLOAD_MASK {
            Some(Val32((2 << TAG_SHIFT) | i))
        } else {
            None
        }
    }

    #[inline]
    pub fn is_default(self) -> bool {
        self.0 >> TAG_SHIFT == 0
    }

    #[inline]
    pub fn is_set(self) -> bool {
        self.0 >> TAG_SHIFT == 1
    }

    #[inline]
    pub fn is_raw(self) -> bool {
        self.0 >> TAG_SHIFT == 2
    }

    #[inline]
    pub fn get(self) -> Option<u32> {
        if self.is_set() {
            Some(self.0 & PAYLOAD_MASK)
        } else {
            None
        }
    }

    #[inline]
    pub fn sym(self) -> Option<Sym> {
        if self.is_raw() {
            Sym::from_raw(self.0 & PAYLOAD_MASK)
        } else {
            None
        }
    }

    /// 打包一个 `PackedVal`（类型由调用方保证 —— 宏按字段类型选定）。
    #[inline]
    pub fn pack<T: PackedVal>(v: T) -> Val32 {
        Val32::set_masked(v.to_u32())
    }

    /// 解出 `PackedVal`（`Set` 态才有意义）。
    #[inline]
    pub fn unpack<T: PackedVal>(self) -> Option<T> {
        self.get().map(T::from_u32)
    }

    #[inline]
    pub fn to_u32(self) -> u32 {
        self.0
    }

    #[inline]
    pub fn from_u32(v: u32) -> Val32 {
        Val32(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sym::Interner;

    #[test]
    fn val_is_eight_bytes_for_u32() {
        // P0 保留 8 B；P1 起按类型改用 Val32
        assert!(std::mem::size_of::<Val<u32>>() <= 8);
    }

    #[test]
    fn val32_is_four_bytes() {
        assert_eq!(std::mem::size_of::<Val32>(), 4);
    }

    #[test]
    fn val32_three_states() {
        let d = Val32::DEFAULT;
        let s = Val32::set(1024).unwrap();
        let mut it = Interner::new();
        let sym = it.intern(b"DG");
        let r = Val32::raw(sym).unwrap();

        assert!(d.is_default() && !d.is_set() && !d.is_raw());
        assert_eq!(d.get(), None);
        assert_eq!(s.get(), Some(1024));
        assert!(!s.is_default());
        assert_eq!(r.sym(), Some(sym));
        assert_eq!(it.resolve(r.sym().unwrap()), b"DG");
    }

    #[test]
    fn val32_rejects_oversized() {
        assert!(Val32::set(PAYLOAD_MASK).is_some());
        assert!(Val32::set(PAYLOAD_MASK + 1).is_none());
    }

    #[test]
    fn packed_roundtrip() {
        let v = Val32::pack(300u16);
        assert_eq!(v.unpack::<u16>(), Some(300));
        let b = Val32::pack(true);
        assert_eq!(b.unpack::<bool>(), Some(true));
        let z = Val32::DEFAULT;
        assert_eq!(z.unpack::<u8>(), None);
    }

    #[test]
    fn val_merge_semantics() {
        // MOD 只覆盖 Set/Raw，不动 Default（与 ims_mem 的 MergeOverwrite 同口径）
        let mut dst = Val::Set(7u32);
        let src: Val<u32> = Val::Default;
        if !matches!(src, Val::Default) {
            dst = src;
        }
        assert_eq!(dst, Val::Set(7));
    }
}

/// A2.5：宽值列的**键**（typed 与原文兜底两种形态都要能当键）。
///
/// 为什么需要它：`MGW.EID` 既是键列、又真的有 raw 兜底（`cdma_ss6.com`）。
/// 若键只能是 `u64`，那些行会以 `key_missing` 被丢掉（D52 不允许）。
/// `Ord` 只要求"全序一致"，`Sym` 的下标序没有语义含义但稳定，够用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WideKey {
    /// 解析成功（`Ip/Eid/Bits/Pair/Spc` 的 u64 形态）
    Num(u128),
    /// 原文兜底（渲染 != 原文，或解析失败）
    Raw(crate::sym::Sym),
}

impl WideKey {
    pub fn as_num(&self) -> Option<u128> {
        match self {
            WideKey::Num(v) => Some(*v),
            WideKey::Raw(_) => None,
        }
    }
}