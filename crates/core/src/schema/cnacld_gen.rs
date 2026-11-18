//! `CNACLD`（号码分析）的宏声明版（P0.4d）：**Pfx 复合键** `(LP, PFX)` 的宽表。
//!
//! 口径与 ims_mem 一致：`pfx` 是 `PfxText`（行内原文 + 键用数字形态），
//! 其余 4 个枚举列走 `(MOC.PARAM)` 标签池，`Bool` 列打包成 0/1。
//!
//! ⚠ `rsc`/`otdispid` 是 `U32`，但行内 `Val32` 只有 30 bit —— 实测值远小于这个上限
//! （RSC 是路由源码，otdispid 是运营商标识），值级对拍会兜住越界。

use crate::mml_table;

mml_table! {
    CNACLD, Cnacld,
    key [lp, pfx],
    lp: U16,
    pfx: PfxText,
    ldn: Text,
    csa: Enum,
    rsc: U32,
    minl: U8,
    maxl: U8,
    clraf: Bool,
    cstp: Enum,
    iac: Bool,
    gain: Enum,
    rdt: U16,
    ecos: Bool,
    spchg: Enum,
    sdescription: Text,
    otdispid: U32,
}

#[cfg(test)]
mod tests {
    use crate::sym::Interner;
    use crate::{scan::Op, LabelCols};

    #[test]
    fn pfx_composite_key_and_enums() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::CnacldTable::new();
        t.set_batch(100);
        t.apply(
            Op::Add,
            b"LP=0,PFX=K'0086755,LDN=\"IMS,to PBX\",CSA=ALL,RSC=1,MINL=8,MAXL=12,CLRAF=YES,CSTP=STP,IAC=NO,GAIN=G1,RDT=3,ECOS=NO,SPCHG=S1,SDESCRIPTION=\"desc\",OTDISPID=5",
            &mut it,
            &mut pools,
        );
        // 同键不同写法：K' 前缀不影响键
        t.apply(
            Op::Set,
            b"LP=0,PFX=0086755,RSC=2",
            &mut it,
            &mut pools,
        );
        assert_eq!(t.len(), 1, "K'0086755 与 0086755 同键");
        let k = (
            0u16,
            crate::frombytes::parse_pfx(b"K'0086755").unwrap(),
        );
        let r = t.get(&k).unwrap();
        assert_eq!(r.rsc(), Some(2), "SET 命中后只改出现的字段");
        assert_eq!(r.minl(), Some(8), "未出现的字段保持不变");
        assert_eq!(r.clraf(), Some(true));
        assert_eq!(r.iac(), Some(false));
        assert_eq!(r.ldn().map(|s| it.resolve(s)), Some(&b"IMS,to PBX"[..]));
        assert_eq!(r.sdescription().map(|s| it.resolve(s)), Some(&b"desc"[..]));
    }
}
