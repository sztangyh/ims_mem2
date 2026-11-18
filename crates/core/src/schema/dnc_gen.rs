//! `DNC` 的**宏声明版**（P0.3）：与 `schema/dnc.rs` 的手写实现同构，作为宏的第一个真实用例。
//!
//! 声明只有 10 行；生成 `DncRow / DncPending / DncColStore / DncHooks / DncTable`。
//! 手写版保留为"参照实现"，两者的等价性由 `tests` 里的对拍守住 ——
//! 宏一旦生成错列序/错类型，这里就会红。
//!
//! 口径：DNC = 号码变换。键 `DCX`；`DCT` 变换类型；`ND` 变换字冠；`ACN` 区域码；
//! `DAI` 分析对象；`DCL` 变换位数；`DCP` 目的码。

use crate::mml_table;

mml_table! {
    DNC, Dnc,
    key [dcx],
    dcx: U16,
    dct: Enum,
    nd: Text,
    acn: Text,
    dai: Enum,
    dcl: U8,
    dcp: U8,
}

#[cfg(test)]
mod tests {
    use crate::schema::dnc as hand;
    use crate::sym::Interner;
    use crate::{scan::Op, LabelCols};

    const LINES: [&str; 3] = [
        "ADD|DCX=100,DCT=DEL,ND=K'140,ACN=K'0,DAI=ALL,DCL=4,DCP=1",
        "ADD|DCX=101,DCT=INS,ND=K'0,DAI=NONE,DCL=2,DCP=3",
        "ADD|DCX=102,DCT=RPL,ND=K'0086755,DAI=UDN,DCL=7", // 缺 DCP
    ];

    /// 同一个输入喂给宏版与手写版，逐行逐字段必须一致。
    #[test]
    fn macro_matches_handwritten() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut gen = super::DncTable::new();
        let mut refi = hand::DncTable::new();
        gen.set_batch(100);
        refi.set_batch(100);

        for line in LINES {
            let (op, body) = line.split_once('|').unwrap();
            let op = Op::parse(op.as_bytes());
            gen.apply(op, body.as_bytes(), &mut it, &mut pools);
            refi.apply(op, body.as_bytes(), &mut it, &mut pools);
        }
        gen.finish();
        refi.finish();

        assert_eq!(gen.len(), refi.len(), "行数一致");
        assert_eq!(gen.len(), 3);
        for k in [100u16, 101, 102] {
            let g = gen.get(&k).expect("宏版应命中");
            let h = refi.get(&k).expect("手写版应命中");
            assert_eq!(g.dcx(), h.dcx(), "DCX");
            assert_eq!(g.dcl(), h.dcl(), "DCL");
            assert_eq!(g.dcp(), h.dcp(), "DCP");
            assert_eq!(g.nd(), h.nd(), "ND");
            assert_eq!(g.acn(), h.acn(), "ACN");
            assert_eq!(g.dct(), h.dct(), "DCT 标签");
            assert_eq!(g.dai(), h.dai(), "DAI 标签");
            // 整行逐列（含枚举/文本）也必须一致
            use crate::vhist::ColStore;
            for c in 0..<super::DncRow as ColStore>::COLS {
                assert_eq!(g.get_col(c), h.get_col(c), "第 {c} 列");
            }
        }
        // 缺 DCP 的那行：Default 而不是 Set(0)
        let r = gen.get(&102).unwrap();
        assert_eq!(r.dcp(), None, "缺参必须是 Default");
        assert_eq!(r.dcl(), Some(7));
    }

    /// 池注册是惰性的：只有 Enum 列进池，且池名按声明序。
    #[test]
    fn pools_are_declared_lazily() {
        use crate::table::TableHooks;
        assert_eq!(super::DncHooks::pools(), &["DNC.DCT", "DNC.DAI"]);
        let mut pools = LabelCols::new();
        let mut t = super::DncTable::new();
        let mut it = Interner::new();
        t.apply(Op::Add, b"DCX=1", &mut it, &mut pools);
        assert_eq!(pools.pool_len(), 2, "首次 apply 时按声明序注册两个池");
    }
}
