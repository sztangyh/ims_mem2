//! `CLRDSN`（主叫甄别）的宏声明版（P0.4d）：键 `(DSP, CLI)`，`cli` 是 `PfxText`。
//!
//! 这是"小表 + 四个枚举 + 两个 Bool"的典型，用来验证同一套宏在小表上的复用。

use crate::mml_table;

mml_table! {
    CLRDSN, Clrdsn,
    key [dsp, cli],
    dsp: U16,
    cli: PfxText,
    dstype: Enum,
    func: Enum,
    dai: Enum,
    mgwf: Enum,
    min: U8,
    max: U8,
    rdcx: U16,
    dcf: Bool,
    clirflag: Bool,
}

#[cfg(test)]
mod tests {
    use crate::sym::Interner;
    use crate::{scan::Op, LabelCols};

    #[test]
    fn key_is_dsp_plus_pfx() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::ClrdsnTable::new();
        t.set_batch(100);
        t.apply(
            Op::Add,
            b"DSP=1,CLI=K'28811182,DSTYPE=NORT,DAI=ALL,FUNC=DSG,MGWF=NMG,MIN=8,MAX=8,DCF=NO,CLIRFLAG=NO",
            &mut it,
            &mut pools,
        );
        t.apply(
            Op::Add,
            b"DSP=1,CLI=K'28811183,DSTYPE=NORT,DAI=ALL",
            &mut it,
            &mut pools,
        );
        assert_eq!(t.len(), 2, "同一 DSP 下不同 CLI 是两行");

        let k1 = (1u16, crate::frombytes::parse_pfx(b"K'28811182").unwrap());
        let k2 = (1u16, crate::frombytes::parse_pfx(b"K'28811183").unwrap());
        assert!(t.get(&k1).is_some());
        assert!(t.get(&k2).is_some());
        let r = t.get(&k1).unwrap();
        assert_eq!(r.min(), Some(8));
        assert_eq!(r.dcf(), Some(false));
        assert_eq!(r.clirflag(), Some(false));
    }
}
