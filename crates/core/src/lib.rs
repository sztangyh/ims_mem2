//! ims_mem2 —— AGCF 后备**全内存**结构化存储（B 方案）。
//!
//! 三条不可动摇的性质（见 `../DESIGN.md`）：
//! 1. **行号稳定**：`rows` 只追加，RMV 置墓碑不移动 → `(tbl,row)` 可长期作引用目标与版本锚点；
//! 2. **单真源**：内存是权威，SQLite/NDJSON 只是可重算的**投影**；
//! 3. **不丢行**：解析失败一律兜底 + 分类计数。
//!
//! 进度：
//! - **P0.1** 原语层：`sym`（单 arena/开放寻址/稀疏 utf8）、`bitset`（1 bit/行）、
//!   `label`（1 B 枚举池）、`val`（`Val<T>` + 4 B `Val32`）、`key_index`（两段式）、
//!   `vhist`（列级差量版本链）
//! - **P0.2** 引擎与端到端：`frombytes`、`scan`（GBK 事件流）、`table`（ADD/MOD/RMV/SET +
//!   墓碑 + 版本链）、`schema::dnc`（首张手写表）、`import`（按 MENAME 归并）
//! - **P7 / D54**：三处**归一化**都在这一层落地，且都能逐字节还原——
//!   `srtcost_scheme`（成本方案池）、`schema::asbr_gen`（SRVRGT 并入 ASBR + URI 拆段）、
//!   `schema::pra_gen`（`PRATKC` 块即行）。展开回扁平行的入口分别是
//!   `SrtcostShim::flat_rows` / `AsbrShim::apply_reg` / `schema::pra_gen::expand`。
//!
//! 全量验收门：`pwsh -File tools\check_tables.ps1`（39 段，见 `../DESIGN.md` §6）。

pub mod bitset;
pub mod circuit;
pub mod frombytes;
pub mod import;
pub mod key_index;
pub mod label;
pub mod mgw;
pub mod num;
pub mod refs;
pub mod refs_gen;
pub mod scan;
pub mod schema;
pub mod schema_gen;
pub mod srtcost_scheme;
pub mod sym;
pub mod trunk;
pub mod table;
pub mod val;
pub mod vhist;
pub mod wire;
pub mod zh;

pub use bitset::Bitset;
pub use ims_mem2_macros::mml_table;
pub use import::{ImportReport, Library, NeStore};
pub use key_index::KeyIndex;
pub use label::{Label, LabelCols};
pub use mgw::{MgwLibrary, MgwLog, MgwNe, MgwRow};
pub use scan::{Event, Op, Params, Scan};
pub use schema::dnc::{DncRow, DncTable};

pub use sym::{Interner, Sym};
pub use table::{Stats, Table, TableApi, TableHooks};
pub use circuit::{BlockBuild, Circuit, PcmBlock};
pub use num::{NumKey, NumStore};
pub use srtcost_scheme::{SchemeStore, SrtScheme};
pub use trunk::{TrunkIndex, TrunkRef};
pub use vhist::Diff;
pub use val::{PackedVal, Val, Val32};
pub use vhist::{as_of, ColStore, ColVal, VHist, Version};
pub use zh::ZhDict;

#[cfg(test)]
mod send_sync_tests {
    /// **这是 Rc<RefCell> → Arc<Mutex> 改造的验收判据**：
    /// Library 必须是 Send + Sync，否则 svc 无法把 Arc<Engine> 分给多个连接线程
    /// （也就谈不上多客户端 / quic 连接循环）。
    #[test]
    fn library_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::Library>();
        assert_send_sync::<crate::mgw::MgwLibrary>();
        assert_send_sync::<std::sync::Arc<crate::Library>>();
    }
}
