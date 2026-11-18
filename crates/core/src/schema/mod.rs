//! 表所在模块：手写参照实现 + 宏声明版。
//!
//! P0.3 起，正式路径走 `dnc_gen`（`mml_table!` 生成）；`dnc` 是**手写参照**，
//! 保留用于"宏生成物 == 手写实现"的持续对拍（见 `dnc_gen::tests`）。

pub mod asbr_gen;
pub mod clrdsn_gen;
pub mod cnacld_gen;
pub mod dnc;
pub mod dnc_gen;
pub mod n7;
pub mod pra_gen;
pub mod srtcost_gen;
pub mod srvrgt_gen;
