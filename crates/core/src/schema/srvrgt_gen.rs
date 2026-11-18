//! `SRVRGT` 的宏声明版（P0.3 的第二张表）：用来验证 **Text 列做键**（`Key = Sym`）这条路径。
//!
//! 口径：业务注册，键 `IMPU`（`sip:+86...@域` 的 URI 原文），值 `SN`。
//! 两者都按原文驻留（`Text`），不做 URI 解析 —— 号码形态的解析留到 P3 的桥接层。

use crate::mml_table;

mml_table! {
    SRVRGT, Srvrgt,
    key [impu],
    impu: Text,
    sn: Text,
}

#[cfg(test)]
mod tests {
    use crate::sym::Interner;
    use crate::{scan::Op, LabelCols};

    #[test]
    fn text_key_roundtrip() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::SrvrgtTable::new();
        t.set_batch(100);
        t.apply(
            Op::Add,
            b"IMPU=\"sip:+8675588908001@gd.ctcims.cn\",SN=\"AGCF63-1\"",
            &mut it,
            &mut pools,
        );
        assert_eq!(t.len(), 1);

        // 用原文查（查询期 intern 一次）
        let k = it.intern(b"sip:+8675588908001@gd.ctcims.cn");
        let row = t.get(&k).expect("Text 键应可命中");
        let sn = row.sn().expect("SN 应驻留");
        assert_eq!(it.resolve(sn), b"AGCF63-1");

        // 同一个 URI 再 ADD 一次（属性相同）→ 覆盖但值不变
        t.apply(
            Op::Add,
            b"IMPU=\"sip:+8675588908001@gd.ctcims.cn\",SN=\"AGCF63-1\"",
            &mut it,
            &mut pools,
        );
        assert_eq!(t.len(), 1, "同键不新增行");
        assert_eq!(t.stats().dup_add, 1);
        assert_eq!(t.stats().unchanged, 1, "值没变记为 unchanged");
        t.end_batch();
        assert_eq!(t.vhist().len(), 0, "重放幂等：值没变就不写版本");

        // 换一个值再 ADD（**下一批**）→ 整行覆盖，批末一条版本
        t.set_batch(200);
        t.apply(
            Op::Add,
            b"IMPU=\"sip:+8675588908001@gd.ctcims.cn\",SN=\"AGCF63-2\"",
            &mut it,
            &mut pools,
        );
        assert_eq!(t.stats().dup_add, 2);
        t.end_batch();
        assert_eq!(t.vhist().len(), 1);
        // D33：批末差分只记**变化列**（旧模型是"整行覆盖 → 整行旧值"，会把没变的列也写进去）
        assert_eq!(t.vhist().versions()[0].cols.len(), 1, "只有 SN 变了 → 只记 1 列");

        // 历史：批 150 时 SN 还是旧值
        let old = t.at(&k, 150).unwrap();
        let sn = old.sn().unwrap();
        assert_eq!(it.resolve(sn), b"AGCF63-1");
        // 当前态是新值
        let now = t.get(&k).unwrap().sn().unwrap();
        assert_eq!(it.resolve(now), b"AGCF63-2");
    }
}

