//! 微基准：纯"取格 + 拼串"（不碰 SQLite）耗时，用来判断导出瓶颈是否在 String 分配。
//!
//! 跑法：`cargo run --release --example bench_render -- <4 个文件>`

use ims_mem2::Library;
use ims_mem2_proj::{cell_of, Cell};
use std::time::Instant;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let t0 = Instant::now();
    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap();
    }
    lib.finish();
    println!("导入 {:.2} s", t0.elapsed().as_secs_f64());

    // 纯渲染：所有表 × 所有行 × 所有列
    let mut out = String::with_capacity(1 << 20);
    let mut cells = 0u64;
    let t1 = Instant::now();
    for ne in &lib.nes {
        let ne_idx = lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap_or(usize::MAX);
        for t in ne.tables() {
            let moc = t.moc();
            let (names, kinds) = t.col_meta();
            let uri_dom = lib.uri_default(ne_idx, moc);
            let n = t.alive_count() as u32;
            for row in 0..n {
                for c in 0..names.len() {
                    let cell = cell_of(
                        &lib,
                        moc,
                        t,
                        row,
                        c as u16,
                        kinds[c],
                        names[c],
                        uri_dom,
                    );
                    out.clear();
                    match cell {
                        Cell::Null => {}
                        Cell::Int(v) => {
                            out.push_str(&v.to_string());
                        }
                        Cell::Text(s) => out.push_str(&s),
                    }
                    cells += 1;
                }
            }
        }
    }
    let secs = t1.elapsed().as_secs_f64();
    println!(
        "纯渲染 {cells} 格：{secs:.2} s（{:.0} ns/格）；上一次渲染结果长度 {}",
        secs * 1e9 / cells as f64,
        out.len()
    );
}
