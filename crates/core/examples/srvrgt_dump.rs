//! `SRVRGT` 的规范形导出（P7 之后 SRVRGT 已并入 ASBR：按 `sn` 非空的行导出）。
//!
//! 输出与 `tools/srvrgt_oracle.py` 完全同构：`ne|impu|sn`。
//!
//! ```powershell
//! cargo run --release --offline --example srvrgt_dump -- ..\agcf_data\ALLME_*.txt
//! ```

use ims_mem2::Library;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: srvrgt_dump <文件1> [文件2 ...]");
        std::process::exit(2);
    }
    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    for (ne_idx, ne) in lib.nes.iter().enumerate() {
        let Some(ne_sym) = ne.name else { continue };
        let ne_name = lib.interner.decode(ne_sym);
        let dom = lib.uri_default(ne_idx, "ASBR");
        for i in 0..ne.asbr.len() as u32 {
            if !ne.asbr.is_alive(i) {
                continue;
            }
            let Some(r) = ne.asbr.row(i) else { continue };
            let Some(sn) = r.sn() else { continue };
            let pui = r
                .pui()
                .map(|x| lib.rehydrate_uri(lib.interner.decode(x), dom))
                .unwrap_or_else(|| "-".into());
            println!("{}|{}|{}", ne_name, pui, lib.interner.decode(sn));
        }
    }
}
