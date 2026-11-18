//! 六张"定表"的存活行数，规范形 `NE|MOC|ALIVE`（与 ims_mem 的 `table_report` 逐条对拍）。
//!
//! ```powershell
//! cargo run --release --offline --example core_dump -- ..\agcf_data\ALLME_*.txt
//! ```

use ims_mem2::Library;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: core_dump <文件1> [文件2 ...]");
        std::process::exit(2);
    }
    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    let mut lines = Vec::new();
    for ne in &lib.nes {
        let name = ne.name.map(|s| lib.interner.decode(s)).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        for (moc, alive) in [
            ("ASBR", ne.asbr.alive_count()),
            ("CNACLD", ne.cnacld.alive_count()),
            ("CLRDSN", ne.clrdsn.alive_count()),
            ("DNC", ne.dnc.alive_count()),
            // SRVRGT 已并入 ASBR（P7）：行数 = sn 非空的行
            ("SRVRGT", {
                let mut n = 0usize;
                for i in 0..ne.asbr.len() as u32 {
                    if !ne.asbr.is_alive(i) {
                        continue;
                    }
                    if let Some(r) = ne.asbr.row(i) {
                        if r.sn().is_some() {
                            n += 1;
                        }
                    }
                }
                n
            }),
            // SRTCOST 已归一化：报"展开后的扁平行数"，与 ims_mem 的 table_report 同口径
            ("SRTCOST", ne.srtcost.flat_rows()),
        ] {
            lines.push(format!("{name}|{moc}|{alive}"));
        }
    }
    lines.sort();
    for l in &lines {
        println!("{l}");
    }
}
