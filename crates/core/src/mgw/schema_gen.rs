//! MGW 表声明入口：`include!` 生成物 `schema_gen_data.rs`。
//!
//! 生成方式：`python tools/port_mgw_schema.py --write`
//! 来源：`C:/dev/ims_ws/data/mgw/moc_schema.json`（59 MOC / 188 列）+ `moc_key.json`（键 / 位图）。
//!
//! 与 AGCF 的 [`crate::schema_gen`] 同构：运行期用 `gen_moc_index()` + `new_gen_table()`
//! 惰性建表，所以 `MgwLibrary` 里只存 `HashMap<usize, Box<dyn TableApi>>`。

include!("schema_gen_data.rs");
