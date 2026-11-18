//! 长尾 MOC（143 张）的声明入口：`include!` 生成物 `schema_gen_data.rs`。
//!
//! 生成方式：`python tools/port_schema.py --in ..\ims_mem\src\schema_gen_data.rs --out crates\core\src\schema_gen_data.rs`
//! （把 ims_mem 已验证的声明按类型映射转过来，而不是重新推导键）。
//!
//! 运行期用 `gen_moc_index()` + `new_gen_table()` 惰性建表，所以 `Library` 里只存
//! `HashMap<usize, Box<dyn TableApi>>`，不随 MOC 数量膨胀代码路径。

include!("schema_gen_data.rs");
