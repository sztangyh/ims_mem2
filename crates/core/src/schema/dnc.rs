//! `DNC`（号码变换）—— 手写 `TableHooks` 实现，作为 `mml_table!` 的参照。
//!
//! 字段口径对齐 ims_mem 的八表之一（`schema/misc.rs` 的 `DNC`）：
//! `DCX` 键 / `DCT` 变换类型 / `ND` 变换字冠 / `ACN` 区域码 / `DAI` 分析对象 /
//! `DCL` 变换位数 / `DCP` 目的码。
//!
//! 值表示：
//! - `DCT`/`DAI` 是 `(MOC,PARAM)` 标签池里的 1 字节枚举 → `ColVal::small(Val32)`
//! - `ND`/`ACN` 是原文驻留（`PfxText` 的文本半） → `ColVal::sym`
//!   （数字伴生列 `*_num` 在历史查询/桥接时按需重解析，见 `vhist.rs` 的说明）
//! - `DCL`/`DCP` 是 `u8`、`DCX` 是 `u16` → `ColVal::small`

use crate::frombytes::{parse_u16, parse_u8, unquote};
use crate::label::{Label, LabelCols};
use crate::scan::Params;
use crate::sym::{Interner, Sym};
use crate::table::{Table, TableHooks};
use crate::val::Val32;
use crate::vhist::{ColStore, ColVal};

pub type DncTable = Table<DncHooks>;

pub const MOC_NAME: &str = "DNC";

/// 列号（编译期常量；版本链与 `ColStore` 都按它寻址）。
pub const C_DCX: u16 = 0;
pub const C_DCT: u16 = 1;
pub const C_ND: u16 = 2;
pub const C_ACN: u16 = 3;
pub const C_DAI: u16 = 4;
pub const C_DCL: u16 = 5;
pub const C_DCP: u16 = 6;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DncRow {
    pub dcx: ColVal,
    pub dct: ColVal,
    pub nd: ColVal,
    pub acn: ColVal,
    pub dai: ColVal,
    pub dcl: ColVal,
    pub dcp: ColVal,
}

impl DncRow {
    #[inline]
    pub fn dcx(&self) -> Option<u16> {
        self.dcx.as_u32().map(|v| v as u16)
    }
    #[inline]
    pub fn dcl(&self) -> Option<u8> {
        self.dcl.as_u32().map(|v| v as u8)
    }
    #[inline]
    pub fn dcp(&self) -> Option<u8> {
        self.dcp.as_u32().map(|v| v as u8)
    }
    /// 枚举列解回标签下标（配合 `LabelCols::label_sym` 得到词）。
    #[inline]
    pub fn dct(&self) -> Option<Label> {
        self.dct
            .as_u32()
            .and_then(|v| u8::try_from(v).ok())
            .and_then(Label::from_u8_0)
    }
    #[inline]
    pub fn dai(&self) -> Option<Label> {
        self.dai
            .as_u32()
            .and_then(|v| u8::try_from(v).ok())
            .and_then(Label::from_u8_0)
    }
    #[inline]
    pub fn nd(&self) -> Option<Sym> {
        self.nd.as_sym()
    }
    #[inline]
    pub fn acn(&self) -> Option<Sym> {
        self.acn.as_sym()
    }
}

impl ColStore for DncRow {
    const COLS: u16 = 7;

    fn get_col(&self, i: u16) -> ColVal {
        match i {
            C_DCX => self.dcx,
            C_DCT => self.dct,
            C_ND => self.nd,
            C_ACN => self.acn,
            C_DAI => self.dai,
            C_DCL => self.dcl,
            _ => self.dcp,
        }
    }

    fn set_col(&mut self, i: u16, v: ColVal) {
        match i {
            C_DCX => self.dcx = v,
            C_DCT => self.dct = v,
            C_ND => self.nd = v,
            C_ACN => self.acn = v,
            C_DAI => self.dai = v,
            C_DCL => self.dcl = v,
            _ => self.dcp = v,
        }
    }
}

/// 解析中间态：三态由 `Option` 表达（`None` = 参数未出现 = MOD 不改动）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DncPending {
    pub dcx: Option<u16>,
    pub dct: Option<Label>,
    pub nd: Option<Sym>,
    pub acn: Option<Sym>,
    pub dai: Option<Label>,
    pub dcl: Option<u8>,
    pub dcp: Option<u8>,
    /// 枚举池溢出等降级次数（P0.2 只计数；P0.3 起进 `LibScn` 分类）
    pub fallbacks: u32,
}

pub struct DncHooks;

#[inline]
fn pack_u16(v: u16) -> ColVal {
    ColVal::small(Val32::set_masked(v as u32))
}

#[inline]
fn pack_u8(v: u8) -> ColVal {
    ColVal::small(Val32::set_masked(v as u32))
}

#[inline]
fn pack_label(l: Label) -> ColVal {
    ColVal::small(Val32::set_masked(l.as_u8() as u32))
}

/// 参数值 → 标签池下标；池满返回 `None` 并计 fallback。
fn label_of(
    it: &mut Interner,
    pools: &mut LabelCols,
    pool: usize,
    v: &[u8],
    fb: &mut u32,
) -> Option<Label> {
    let s = it.intern(unquote(v));
    match pools.label_of(pool, s) {
        Some(l) => Some(l),
        None => {
            *fb += 1;
            None
        }
    }
}

impl TableHooks for DncHooks {
    type Row = DncRow;
    type Pending = DncPending;
    type Key = u16;

    const MOC: &'static str = MOC_NAME;
    const KEYED: bool = true;
    const COL_NAMES: &'static [&'static str] = &["DCX", "DCT", "ND", "ACN", "DAI", "DCL", "DCP"];
    const COL_KINDS: &'static [u8] = b"nettent";
    const KEY_COLS: &'static [u16] = &[0];
    const COL_GROUPS: &'static [(u16, u8)] = &[];

    fn pools() -> &'static [&'static str] {
        &["DNC.DCT", "DNC.DAI"]
    }

    fn parse(
        body: &[u8],
        it: &mut Interner,
        pools: &mut LabelCols,
        ids: &[usize],
        p: &mut Self::Pending,
    ) {
        for (k, v) in Params::new(body) {
            if k.eq_ignore_ascii_case(b"DCX") {
                p.dcx = parse_u16(v);
            } else if k.eq_ignore_ascii_case(b"DCT") {
                p.dct = label_of(it, pools, ids[0], v, &mut p.fallbacks);
            } else if k.eq_ignore_ascii_case(b"ND") {
                p.nd = Some(it.intern(unquote(v)));
            } else if k.eq_ignore_ascii_case(b"ACN") {
                p.acn = Some(it.intern(unquote(v)));
            } else if k.eq_ignore_ascii_case(b"DAI") {
                p.dai = label_of(it, pools, ids[1], v, &mut p.fallbacks);
            } else if k.eq_ignore_ascii_case(b"DCL") {
                p.dcl = parse_u8(v);
            } else if k.eq_ignore_ascii_case(b"DCP") {
                p.dcp = parse_u8(v);
            }
        }
    }

    fn key_of(p: &Self::Pending) -> Option<u16> {
        p.dcx
    }

    fn build(p: &Self::Pending, _it: &Interner) -> DncRow {
        DncRow {
            dcx: p.dcx.map(pack_u16).unwrap_or_default(),
            dct: p.dct.map(pack_label).unwrap_or_default(),
            nd: p.nd.map(|s| ColVal::sym(Some(s))).unwrap_or_default(),
            acn: p.acn.map(|s| ColVal::sym(Some(s))).unwrap_or_default(),
            dai: p.dai.map(pack_label).unwrap_or_default(),
            dcl: p.dcl.map(pack_u8).unwrap_or_default(),
            dcp: p.dcp.map(pack_u8).unwrap_or_default(),
        }
    }

    fn merge(row: &mut Self::Row, p: &Self::Pending, _it: &Interner) -> Vec<(u16, ColVal)> {
        let mut out = Vec::new();
        let set = |col: u16, old: ColVal, new: ColVal, out: &mut Vec<(u16, ColVal)>| {
            if old != new {
                out.push((col, old));
            }
        };
        if let Some(v) = p.dcx {
            set(C_DCX, row.dcx, pack_u16(v), &mut out);
        }
        if let Some(v) = p.dct {
            set(C_DCT, row.dct, pack_label(v), &mut out);
        }
        if let Some(v) = p.nd {
            set(C_ND, row.nd, ColVal::sym(Some(v)), &mut out);
        }
        if let Some(v) = p.acn {
            set(C_ACN, row.acn, ColVal::sym(Some(v)), &mut out);
        }
        if let Some(v) = p.dai {
            set(C_DAI, row.dai, pack_label(v), &mut out);
        }
        if let Some(v) = p.dcl {
            set(C_DCL, row.dcl, pack_u8(v), &mut out);
        }
        if let Some(v) = p.dcp {
            set(C_DCP, row.dcp, pack_u8(v), &mut out);
        }
        // 写回
        for (c, _) in &out {
            let nv = match *c {
                C_DCX => p.dcx.map(pack_u16),
                C_DCT => p.dct.map(pack_label),
                C_ND => p.nd.map(|s| ColVal::sym(Some(s))),
                C_ACN => p.acn.map(|s| ColVal::sym(Some(s))),
                C_DAI => p.dai.map(pack_label),
                C_DCL => p.dcl.map(pack_u8),
                _ => p.dcp.map(pack_u8),
            };
            if let Some(v) = nv {
                row.set_col(*c, v);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Op;

    fn sample() -> (&'static str, &'static str) {
        (
            "ADD",
            "DCX=100,DCT=DEL,ND=K'140,ACN=K'0,DAI=ALL,DCL=4,DCP=1",
        )
    }

    #[test]
    fn parse_and_build() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = DncTable::new();
        t.set_batch(100);
        let (op, body) = sample();
        t.apply(Op::parse(op.as_bytes()), body.as_bytes(), &mut it, &mut pools);

        assert_eq!(t.len(), 1);
        let r = t.get(&100).expect("DCX=100 应可按键命中");
        assert_eq!(r.dcx(), Some(100));
        assert_eq!(r.dcl(), Some(4));
        assert_eq!(r.dcp(), Some(1));
        let nd = r.nd().expect("ND 应驻留原文");
        assert_eq!(it.resolve(nd), b"K'140");
        let acn = r.acn().expect("ACN 应驻留原文");
        assert_eq!(it.resolve(acn), b"K'0");
        // 枚举解回词
        let dct = r.dct().expect("DCT 应命中标签");
        let pdct = pools.register("DNC.DCT");
        let word = pools.label_sym(pdct, dct).unwrap();
        assert_eq!(it.resolve(word), b"DEL");
        let dai = r.dai().expect("DAI 应命中标签");
        let pdai = pools.register("DNC.DAI");
        let word = pools.label_sym(pdai, dai).unwrap();
        assert_eq!(it.resolve(word), b"ALL");
        assert_eq!(pools.pool_sizes(), vec![("DNC.DCT", 1), ("DNC.DAI", 1)]);
    }

    #[test]
    fn mod_only_touches_present_fields() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = DncTable::new();
        t.set_batch(100);
        let (op, body) = sample();
        t.apply(Op::parse(op.as_bytes()), body.as_bytes(), &mut it, &mut pools);

        t.set_batch(200);
        t.apply(Op::Mod, b"DCX=100,DCL=6", &mut it, &mut pools);
        t.end_batch(); // 批末落盘版本

        let r = t.get(&100).unwrap();
        assert_eq!(r.dcl(), Some(6));
        assert_eq!(r.dcp(), Some(1), "未出现的字段保持不变");
        assert_eq!(t.vhist().len(), 1, "变一列 → 一条版本");
        assert_eq!(&*t.vhist().versions()[0].cols, &[C_DCL][..]);

        // 历史：批 150 时 DCL 还是 4
        let old = t.at(&100, 150).unwrap();
        assert_eq!(old.dcl(), Some(4));
        assert_eq!(old.dcp(), Some(1));
        // 当前态不受影响
        assert_eq!(t.get(&100).unwrap().dcl(), Some(6));
    }

    #[test]
    fn enum_pool_overflow_counts_fallback() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut p = DncPending::default();
        let pool = pools.register("DNC.DCT");
        // 灌满 255 个词
        for i in 0..255u32 {
            let s = it.intern(format!("V{i}").as_bytes());
            assert!(pools.label_of(pool, s).is_some());
        }
        let v = b"OVERFLOW";
        let got = label_of(&mut it, &mut pools, pool, v, &mut p.fallbacks);
        assert_eq!(got, None);
        assert_eq!(p.fallbacks, 1);
    }
}

