//! MGW 投影：43 张表 → `temp/mgw_proj.db` 的 `mgw_<moc>` 前缀表（命名空间见 `proj::mgw` 头注释）。
//!
//! ```powershell
//! cargo run -p ims_mem2_proj --release --offline --example mgw_export -- C:\dev\ims_ws\data\mgw
//! ```

use ims_mem2::mgw::MgwLibrary;
use ims_mem2_proj::{export_mgw, mgw_proj_counts};

fn collect(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(path)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    for e in entries {
        if e.is_dir() {
            collect(&e, out);
        } else if e.extension().map(|x| x.eq_ignore_ascii_case("txt")).unwrap_or(false) {
            out.push(e);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: mgw_export <MGW 日志目录或 .txt...>");
        std::process::exit(2);
    }
    let _ = std::fs::create_dir_all("temp");
    let db = "temp/mgw_proj.db";
    let _ = std::fs::remove_file(db);

    let mut lib = MgwLibrary::new();
    if let Ok(b) = std::fs::read("mgw_ne_map.tsv") {
        let n = lib.load_ne_map(&b);
        println!("ne_map: {n} 条");
    }
    let mut files = Vec::new();
    for a in &args {
        collect(std::path::Path::new(a), &mut files);
    }
    let t0 = std::time::Instant::now();
    for f in &files {
        if let Ok(b) = std::fs::read(f) {
            lib.import_bytes(&b);
        }
    }
    lib.finish();
    let t_imp = t0.elapsed();

    let mut mem_rows = 0u64;
    for ne in &lib.nes {
        for (mi, t) in &ne.tables {
            let _ = mi;
            mem_rows += t.alive_count() as u64;
        }
    }

    let t1 = std::time::Instant::now();
    let st = export_mgw(&lib, db).expect("MGW 投影失败");
    let t_exp = t1.elapsed();

    let con = rusqlite::Connection::open(db).unwrap();
    let counts = mgw_proj_counts(&con).unwrap();
    let total: i64 = counts.iter().map(|x| x.1).sum();
    let bytes = std::fs::metadata(db).map(|m| m.len()).unwrap_or(0);
    println!(
        "导入 {:.0} ms / 导出 {:.0} ms\n表 {} 张 / 列 {} / 内存存活 {} 行 / 库内 {} 行 / DB {:.1} MB",
        t_imp.as_secs_f64() * 1000.0,
        t_exp.as_secs_f64() * 1000.0,
        st.tables,
        st.cols,
        mem_rows,
        total,
        bytes as f64 / 1048576.0
    );
    if mem_rows != total as u64 {
        eprintln!("x 内存 {mem_rows} 行 != 库内 {total} 行");
        std::process::exit(1);
    }
    for (n, c) in &counts {
        println!("{n}\t{c}");
    }
}
