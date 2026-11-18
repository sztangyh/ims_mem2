//! MGW 导入 + 逐表 dump（对拍 `ims_core` 的 `mgw_library.db`）。
//!
//! ```powershell
//! cargo run -p ims_mem2 --release --offline --example mgw_dump -- <目录或文件...>
//! ```
//!
//! 输入可以是**目录**（递归取 `*.txt`，按目录名排序 = 批次序）或单个文件。
//! 网元映射读仓内 `mgw_ne_map.tsv`；没有也能跑（网元名退化成 `IP:port`）。
//!
//! 输出（stdout，每行一条**存活**行，排序后）：
//!
//! ```text
//! MOC \t ne_short \t k1=v1<US>k2=v2 \t col=val<US>col=val
//! ```
//!
//! - 键按 `KEY_COLS` 的声明序；非键列**按列名排序**（两侧口径一致即可）
//! - `<US>` = `\x1f`（值里不含它，可以直接 `split`）
//! - `NULL` 列不输出（与投影的 `Cell::Null` 口径一致）
//! - `--stats` 只打每张表的 `(MOC, 行数, 存活数)`，不逐行输出

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ims_mem2::mgw::MgwLibrary;

const US: char = '\u{1f}';

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
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
    let stats_only = args.iter().any(|a| a == "--stats");
    let io_mode = args.iter().any(|a| a == "--io");
    let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if paths.is_empty() {
        eprintln!("用法: mgw_dump [--stats|--io] <目录或 .txt 文件...>");
        std::process::exit(2);
    }

    let mut lib = MgwLibrary::new();
    if let Ok(b) = std::fs::read("mgw_ne_map.tsv") {
        let n = lib.load_ne_map(&b);
        eprintln!("ne_map: {n} 条");
    } else {
        eprintln!("ne_map: 未找到 mgw_ne_map.tsv（网元名将退化为 IP:port）");
    }

    let mut files = Vec::new();
    for p in &paths {
        collect(Path::new(p), &mut files);
    }
    let t0 = std::time::Instant::now();
    let mut lines = 0u64;
    for f in &files {
        match std::fs::read(f) {
            Ok(b) => {
                if let Some(n) = lib.import_bytes(&b) {
                    lines += n as u64;
                } else {
                    eprintln!("  跳过（无 DSP CFG 报告）: {}", f.display());
                }
            }
            Err(e) => eprintln!("  读不了 {}: {e}", f.display()),
        }
    }
    let t_imp = t0.elapsed();
    let t1 = std::time::Instant::now();
    lib.finish();
    let t_fin = t1.elapsed();

    // 存储占用：表本身 + 索引 + 版本链（`TableApi::bytes`）+ arena
    // TableApi::bytes() 已经是"行 + 墓碑 + 键索引 + 版本链"的合计
    let mut tbl_bytes = 0usize;
    let mut rows = 0u64;
    for ne in &lib.nes {
        for t in ne.tables.values() {
            tbl_bytes += t.bytes();
            rows += t.alive_count() as u64;
        }
    }
    let (rss, peak) = proc_mem_kb();
    eprintln!(
        "文件 {} 个 / 配置行 {lines} / 表 {} 张 / 存活 {rows} 行",
        files.len(),
        lib.stats().len()
    );
    eprintln!(
        "导入 {:.0} ms（解析+合并+写表）/ finish {:.0} ms",
        t_imp.as_secs_f64() * 1000.0,
        t_fin.as_secs_f64() * 1000.0
    );
    eprintln!(
        "存储：表合计（行+索引+版本链）{:.2} MB / arena {:.3} MB / RSS 当前 {} KB 峰值 {} KB",
        tbl_bytes as f64 / 1048576.0,
        lib.interner.storage_bytes() as f64 / 1048576.0,
        rss,
        peak
    );

    if io_mode {
        // ---- Mgw：每个 MGW 一个对象（key = mid）
        // 字段与 `spec/io_spec.json` 的 Mgw 一一对应：
        //   mid / name / site / controller / addr / link_counts
        let link_fams = [
            "Q921LNK", "V5E1", "V5LNK", "MTP2LNK", "L2UALNK", "H248LNK", "N7LNK",
        ];
        let mut out: Vec<String> = Vec::new();
        for ne in &lib.nes {
            let cnt = |moc: &str| -> usize {
                ims_mem2::mgw::schema_gen::gen_moc_index(moc.as_bytes())
                    .and_then(|i| ne.tables.get(&i))
                    .map(|t| t.alive_count())
                    .unwrap_or(0)
            };
            // controller = 该 MGW 的 MGC（MSS）地址，取 MGC 表第一行的 MID
            let controller = cnt("MGC");
            let mgc_mid = ims_mem2::mgw::schema_gen::gen_moc_index(b"MGC")
                .and_then(|i| ne.tables.get(&i))
                .and_then(|t| {
                    let (names, _) = t.col_meta();
                    let c = names.iter().position(|n| *n == "MID")? as u16;
                    let r = (0..t.len() as u32).find(|r| t.is_alive(*r))?;
                    t.col_sym(r, c)
                        .map(|s| String::from_utf8_lossy(lib.interner.resolve(s)).to_string())
                })
                .unwrap_or_default();
            let counts: Vec<String> = link_fams
                .iter()
                .filter(|m| cnt(m) > 0)
                .map(|m| format!("\"{}\":{}", m.to_ascii_lowercase(), cnt(m)))
                .collect();
            out.push(format!(
                "Mgw\t{}\t{}\t{}\t{}\t{}\t{{{}}}",
                ne.mid,
                ne.name,
                ne.short,
                if controller == 0 { String::new() } else { mgc_mid },
                ne.mid,
                counts.join(",")
            ));
        }
        // ---- MgwLink：MGW 侧的 IP 链路（只有带对端 IP 的两族：L2UALNK / H248LNK）
        // key = (mgw_mid, kind, id)；local/remote 是该族的 LOCALIP1/REMOTEIP1（L2UA）或
        // LOCALIP/PEERIP（H248）。
        // `id` 的取法：l2ua 必须带 `PROTYPE`（IUA/M2UA），否则 `(ne, LNKNO)` 会撞
        // （实测 6 组：同一 LNKNO 下 IUA 与 M2UA 各一条）；h248 的 `LINKID` 每网元唯一。
        for (moc, kind, idcols, lcol, rcol) in [
            ("L2UALNK", "l2ua", &["PROTYPE", "LNKNO"][..], "LOCALIP1", "REMOTEIP1"),
            ("H248LNK", "h248", &["LINKID"][..], "LOCALIP", "PEERIP"),
        ] {
            let Some(mi) = ims_mem2::mgw::schema_gen::gen_moc_index(moc.as_bytes()) else {
                continue;
            };
            for ne in &lib.nes {
                let Some(t) = ne.tables.get(&mi) else { continue };
                let (names, _) = t.col_meta();
                let idx = |n: &str| names.iter().position(|x| *x == n).map(|x| x as u16);
                let ids: Vec<u16> = idcols.iter().filter_map(|c| idx(c)).collect();
                let (Some(li), Some(ri)) = (idx(lcol), idx(rcol)) else {
                    continue;
                };
                if ids.len() != idcols.len() {
                    continue;
                }
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) {
                        continue;
                    }
                    let get = |c: u16| {
                        t.col_sym(row, c)
                            .map(|s| String::from_utf8_lossy(lib.interner.resolve(s)).to_string())
                            .unwrap_or_default()
                    };
                    let id: Vec<String> = ids.iter().map(|c| get(*c)).collect();
                    out.push(format!(
                        "MgwLink\t{}\t{}\t{}\t{}\t{}",
                        ne.mid,
                        kind,
                        id.join("/"),
                        get(li),
                        get(ri)
                    ));
                }
            }
        }
        // ---- 4 个"表即对象"的 IO：Q921Link / V5Link / Tdmiu / Board
        // 格式与 `--stats` 之外的 dump 一致：`IO \t ne \t 键 \t 列`（US = \x1f）。
        for (io_name, moc) in [
            ("Q921Link", "Q921LNK"),
            ("V5Link", "V5LNK"),
            ("Tdmiu", "TDMIU"),
            ("Board", "BRD"),
        ] {
            let Some(mi) = ims_mem2::mgw::schema_gen::gen_moc_index(moc.as_bytes()) else {
                continue;
            };
            for ne in &lib.nes {
                let Some(t) = ne.tables.get(&mi) else { continue };
                let (names, _) = t.col_meta();
                let kcols = t.key_cols();
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) {
                        continue;
                    }
                    let get = |c: u16| {
                        t.col_sym(row, c)
                            .map(|s| String::from_utf8_lossy(lib.interner.resolve(s)).to_string())
                    };
                    let ks: Vec<String> = kcols
                        .iter()
                        .map(|c| format!("{}={}", names[*c as usize], get(*c).unwrap_or_default()))
                        .collect();
                    let mut cs: BTreeMap<&str, String> = BTreeMap::new();
                    for (ci, n) in names.iter().enumerate() {
                        let ci = ci as u16;
                        if kcols.contains(&ci) {
                            continue;
                        }
                        if let Some(v) = get(ci) {
                            cs.insert(n, v);
                        }
                    }
                    let cols: Vec<String> = cs.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    out.push(format!(
                        "{io_name}\t{}\t{}\t{}",
                        ne.short,
                        ks.join(&US.to_string()),
                        cols.join(&US.to_string())
                    ));
                }
            }
        }
        out.sort();
        for l in &out {
            println!("{l}");
        }
        eprintln!("IO 行 {} 条", out.len());
        return;
    }

    if stats_only {
        let mut tot = 0u64;
        for (moc, len, alive) in lib.stats() {
            println!("{moc}\t{len}\t{alive}");
            tot += alive;
        }
        eprintln!("合计存活 {tot} 行");
        return;
    }

    // 逐表逐行 dump（排序后输出，便于两侧集合比对）
    let mut out: Vec<String> = Vec::new();
    for ne in &lib.nes {
        for (mi, t) in &ne.tables {
            let moc = ims_mem2::mgw::MgwLibrary::mocs()[*mi];
            let (names, _kinds) = t.col_meta();
            let kcols = t.key_cols();
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                let mut ks: Vec<String> = Vec::new();
                for c in kcols {
                    let v = t
                        .col_sym(row, *c)
                        .map(|s| String::from_utf8_lossy(lib.interner.resolve(s)).to_string())
                        .unwrap_or_default();
                    ks.push(format!("{}={}", names[*c as usize], v));
                }
                let mut cs: BTreeMap<&str, String> = BTreeMap::new();
                for (ci, name) in names.iter().enumerate() {
                    let ci = ci as u16;
                    if kcols.contains(&ci) {
                        continue;
                    }
                    if let Some(s) = t.col_sym(row, ci) {
                        cs.insert(name, String::from_utf8_lossy(lib.interner.resolve(s)).to_string());
                    }
                }
                let cols: Vec<String> = cs.iter().map(|(k, v)| format!("{k}={v}")).collect();
                out.push(format!(
                    "{moc}\t{}\t{}\t{}",
                    ne.short,
                    ks.join(&US.to_string()),
                    cols.join(&US.to_string())
                ));
            }
        }
    }
    out.sort();
    for l in &out {
        println!("{l}");
    }
    eprintln!("dump {} 行", out.len());
}

// ---------- 进程内存（Windows：K32GetProcessMemoryInfo，与 dnc_stats 同一实现） ----------

#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, cb: u32) -> i32;
}

/// `(当前 RSS, 峰值 RSS)`，单位 KB。失败时 `(0, 0)`。
fn proc_mem_kb() -> (usize, usize) {
    let mut c = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: 本进程句柄 + 合法结构体指针 + 正确长度
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    if ok == 0 {
        (0, 0)
    } else {
        (c.working_set_size / 1024, c.peak_working_set_size / 1024)
    }
}
