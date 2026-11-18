//! 长尾表对账用：每个网元每张长尾表的存活行数，规范形 `NE|MOC|ALIVE`。
//!
//! ```powershell
//! cargo run --release --offline --example gen_dump -- ..\agcf_data\ALLME_*.txt
//! ```

use ims_mem2::Library;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: gen_dump <文件1> [文件2 ...]");
        std::process::exit(2);
    }
    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    let mut lines = Vec::new();
    let mut total = 0usize;
    for ne in &lib.nes {
        let name = ne
            .name
            .map(|s| lib.interner.decode(s))
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        for (moc, rows, alive, _vers, _bytes) in ne.gen_report() {
            total += alive;
            lines.push(format!("{name}|{moc}|{alive}|{rows}"));
        }
    }
    lines.sort();
    for l in &lines {
        println!("{l}");
    }
    eprintln!("长尾表 {} 张（每网元计），存活合计 {} 行", lines.len(), total);
}
