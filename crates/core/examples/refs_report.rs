//! Ref 物化报告（P3）：跑真实 6 文件，打印每条边的命中/悬空，并做语义抽检。
//!
//! ```powershell
//! cargo run --release --offline --example refs_report -- ..\agcf_data\ALLME_*.txt
//! ```

use ims_mem2::refs_gen::EDGES;
use ims_mem2::Library;

#[repr(C)]
#[derive(Default)]
struct Pmc {
    cb: u32,
    pf: u32,
    peak_ws: usize,
    ws: usize,
    q_peak_paged: usize,
    q_paged: usize,
    q_peak_np: usize,
    q_np: usize,
    pagefile: usize,
    peak_pagefile: usize,
}
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn K32GetProcessMemoryInfo(p: isize, c: *mut Pmc, cb: u32) -> i32;
}
fn rss_kb() -> i64 {
    let mut c = Pmc { cb: std::mem::size_of::<Pmc>() as u32, ..Default::default() };
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    if ok == 0 { 0 } else { c.ws as i64 / 1024 }
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: refs_report <文件1> [文件2 ...]");
        std::process::exit(2);
    }
    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();
    let rss0 = rss_kb();
    let t0 = std::time::Instant::now();
    lib.resolve_refs();
    let dt = t0.elapsed().as_secs_f64();
    let rss1 = rss_kb();
    println!("Ref 物化开销：RSS {:.1} MB → {:.1} MB（+{:.1} MB），耗时 {dt:.2}s", rss0 as f64/1024.0, rss1 as f64/1024.0, (rss1-rss0) as f64/1024.0);

    let idx_entries: usize = lib.nes.iter().map(|n| n.refs.index_entries()).sum();
    println!("目标索引条目合计 {idx_entries}（逐条物化的话是 444 万条指针）");
    let mut tot = (0u64, 0u64, 0u64);
    let mut per_edge = vec![(0u64, 0u64); EDGES.len()];
    for ne in &lib.nes {
        for (i, w) in per_edge.iter_mut().enumerate() {
            w.0 += ne.refs.resolved[i];
            w.1 += ne.refs.dangling[i];
        }
        let (_, r, d, m) = ne.refs.totals();
        tot.0 += r;
        tot.1 += d;
        tot.2 += m;
    }

    println!("边总数 {}（moc_ref.json 的 match=eq 且两端已建模）", EDGES.len());
    println!(
        "命中 {} / 悬空 {} / 缺表 {}；有命中的边 {} 条",
        tot.0,
        tot.1,
        tot.2,
        per_edge.iter().filter(|x| x.0 > 0).count()
    );

    let mut idx: Vec<usize> = (0..EDGES.len()).collect();
    idx.sort_by_key(|&i| std::cmp::Reverse(per_edge[i].0));
    // 缺口 10：把"0 命中但源行数不为 0"的边也列出来（这些就是死边）
    let mut dead: Vec<(usize, u64)> = (0..EDGES.len())
        .map(|i| (i, per_edge[i].0))
        .filter(|(i, h)| *h == 0 && per_edge[*i].1 > 0)
        .collect();
    dead.sort_by_key(|(_, miss)| std::cmp::Reverse(*miss));
    if !dead.is_empty() {
        println!("\n0 命中的边（死边）{} 条：", dead.len());
        for (i, miss) in dead.iter().take(10) {
            println!("  {:<48} 命中 0  悬空 {:>7}", EDGES[*i].label, miss);
        }
    }
    println!("\n命中数前 12 条边：");
    let top = if std::env::var("REFS_ALL").is_ok() {
        idx.len()
    } else {
        12
    };
    for &i in idx.iter().take(top) {
        let (r, d) = per_edge[i];
        if r == 0 {
            break;
        }
        println!(
            "  {:38} 命中 {:7}  悬空 {:7}  ({:.1}%)",
            EDGES[i].label,
            r,
            d,
            r as f64 * 100.0 / (r + d).max(1) as f64
        );
    }

    // 语义抽检：ASBR 的 TRUNKGP/TGN → PRATG.TG，按 DID 看命中率
    // （带中继群的 DID 应全命中；ESL/SIPPBX 这类没有 TG 的应全悬空）
    let ei = EDGES
        .iter()
        .position(|e| e.from == "ASBR" && e.to == "PRATG")
        .expect("应有 ASBR→PRATG 的边");
    let pdraw = lib.pools.find("ASBR.DID").unwrap_or(usize::MAX);
    println!("\n抽检 {}：按 DID 分组的命中/悬空", EDGES[ei].label);
    let mut dist: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for ne in &lib.nes {
        for i in 0..ne.asbr.len() as u32 {
            if !ne.asbr.is_alive(i) {
                continue;
            }
            let Some(r) = ne.asbr.row(i) else { continue };
            let w = r
                .did()
                .and_then(|l| lib.pools.label_sym(pdraw, l))
                .map(|s| lib.interner.decode(s))
                .unwrap_or_else(|| "-".into());
            let e = dist.entry(w).or_default();
            if lib.ref_of(lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap(), ei, i).is_some() {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
    }
    for (k, (hit, miss)) in dist {
        println!("  {k:8} 命中 {hit:7}  悬空 {miss:7}");
    }
}
