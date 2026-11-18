//! P0.2 端到端：把真实后备文件跑一遍，打印 DNC 表的行数/统计/占用。
//!
//! ```powershell
//! cargo run --release --offline --example dnc_stats -- ..\agcf_data\ALLME_*.txt
//! ```

use std::time::Instant;

use ims_mem2::Library;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: dnc_stats <文件1> [文件2 ...]");
        std::process::exit(2);
    }

    let mut lib = Library::new();
    // A4：装载冻结的提升列缺省清单（没有这个文件 = 旧行为：首见即缺省）
    let mut preset_n = 0usize;
    if let Ok(b) = std::fs::read("defaults.tsv") {
        preset_n = lib.load_defaults(&b);
    }
    let t_all = Instant::now();
    let mut read_total = 0f64;
    let trace = std::env::var("DSH_RSS_TRACE").is_ok();
    for p in &files {
        let mb = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / 1048576.0;
        let t = Instant::now();
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        read_total += ms;
        println!("{p}  {mb:6.1} MB  import {ms:8.2} ms");
        if trace {
            let (rss, peak) = proc_mem_kb();
            println!("   [rss] 导入完第 {} 个: 当前 {rss} KB / 峰值 {peak} KB", p);
        }
    }
    if trace {
        let (rss, peak) = proc_mem_kb();
        println!("   [rss] finish 之前: 当前 {rss} KB / 峰值 {peak} KB");
    }
    lib.finish();
    if trace {
        let (rss, peak) = proc_mem_kb();
        println!("   [rss] finish 之后: 当前 {rss} KB / 峰值 {peak} KB");
    }

    println!("\n===== 汇总 =====");
    println!("导入总墙钟: {:.3} s（其中解析 {:.1} ms）", t_all.elapsed().as_secs_f64(), read_total);
    println!("report: {:?}", lib.report);
    println!(
        "batch_no: {:?}   sym: {} 个 distinct / arena {:.2} MB / 索引 {:.2} MB / utf8缓存 {} 条",
        lib.batch_no,
        lib.interner.len(),
        lib.interner.storage_bytes() as f64 / 1048576.0,
        lib.interner.index_bytes() as f64 / 1048576.0,
        lib.interner.utf8_count()
    );
    println!("pools: {} 个 / 约 {:.3} MB", lib.pools.pool_len(), lib.pools.bytes() as f64 / 1048576.0);

    println!("\n---- 每个 NeStore 的 DNC ----");
    let mut total_rows = 0usize;
    let mut total_alive = 0usize;
    for ne in &lib.nes {
        let name = ne
            .name
            .map(|s| String::from_utf8_lossy(lib.interner.resolve(s)).into_owned())
            .unwrap_or_else(|| format!("<meid={} 无名>", ne.meid));
        let st = ne.dnc.stats();
        total_rows += ne.dnc.len();
        total_alive += ne.dnc.alive_count();
        println!(
            "{name:8} meid={:4} batch={} rows={:5} alive={:5} | add={:5} dup={:3} mod={:4} mod_no_add={:2} rmv={:4} rmv_miss={:2} key_missing={:2} unchanged={:3} versions={:4} | {:6.1} KB",
            ne.meid,
            ne.batch,
            ne.dnc.len(),
            ne.dnc.alive_count(),
            st.adds,
            st.dup_add,
            st.mods,
            st.mod_no_add,
            st.rmvs,
            st.rmv_miss,
            st.key_missing,
            st.unchanged,
            st.versions,
            ne.dnc.bytes() as f64 / 1024.0
        );
    }
    println!("\nDNC 合计: rows={total_rows} alive={total_alive} 占用={:.2} MB（含索引/版本链）",
        lib.dnc_bytes() as f64 / 1048576.0);

    println!("\n---- 每个 NeStore 的 SRVRGT（Text 做键）----");
    // SRVRGT 已并入 ASBR（P7）：行数 = sn 非空的行
    let mut srv_rows = 0usize;
    let mut srv_alive = 0usize;
    for ne in &lib.nes {
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
        srv_rows += n;
        srv_alive += n;
    }
    println!("SRVRGT（并入 ASBR，sn 非空）合计: rows={srv_rows} alive={srv_alive}");
    {
        // 缺口 10：SRVRGT 的引用检查（原先那条 `SRVRGT.IMPU->ASBR.PUI` 是死边）
        let (mut applied, mut orphan) = (0u64, 0u64);
        for ne in &lib.nes {
            applied += ne.asbr.reg_applied;
            orphan += ne.asbr.orphan_reg;
        }
        println!("SRVRGT→ASBR 注册：落盘 {applied} / 孤儿 {orphan}（必须为 0）");
    }

    println!("\n---- 每个 NeStore 的 SRTCOST（Pfx 复合键 + SET 多行）----");
    let mut sc_rows = 0usize;
    let mut sc_alive = 0usize;
    for ne in &lib.nes {
        let name = ne
            .name
            .map(|s| String::from_utf8_lossy(lib.interner.resolve(s)).into_owned())
            .unwrap_or_else(|| format!("<meid={} 无名>", ne.meid));
        sc_rows += ne.srtcost.len();
        sc_alive += ne.srtcost.alive_count();
        if ne.srtcost.is_empty() {
            continue;
        }
        let st = ne.srtcost.stats();
        println!(
            "{name:8} rows={:6} alive={:6} | set={:6} dup={:3} mod={:4} rmv={:3} key_missing={:4} unchanged={:4} versions={:4}",
            ne.srtcost.len(),
            ne.srtcost.alive_count(),
            st.sets,
            st.dup_add,
            st.mods,
            st.rmvs,
            st.key_missing,
            st.unchanged,
            st.versions
        );
    }
    println!("SRTCOST 合计: rows={sc_rows} alive={sc_alive}");

    println!("\n---- 每个 NeStore 的 PRATKC（块即行：1 行 = 1 个 PCM 块，D54）----");
    let (mut tkc_rows, mut tkc_flat, mut tkc_exc, mut tkc_bytes) = (0usize, 0usize, 0u64, 0usize);
    let mut tkc_unknown = 0u64;
    for ne in &lib.nes {
        let name = ne
            .name
            .map(|s| String::from_utf8_lossy(lib.interner.resolve(s)).into_owned())
            .unwrap_or_else(|| format!("<meid={} 无名>", ne.meid));
        tkc_rows += ne.pra_tkc.len();
        tkc_flat += ne.pra_tkc.flat_rows();
        tkc_exc += ne.pra_tkc.exceptions();
        tkc_unknown += ne.pra_tkc.unknown_params() + ne.pra_tkc.cs_odd();
        tkc_bytes += ne.pra_tkc.bytes();
        if ne.pra_tkc.is_empty() {
            continue;
        }
        println!(
            "{name:8} 块行={:5} 展开电路={:6} 例外={:3} | {:6.1} KB",
            ne.pra_tkc.len(),
            ne.pra_tkc.flat_rows(),
            ne.pra_tkc.exceptions(),
            ne.pra_tkc.bytes() as f64 / 1024.0
        );
    }
    println!(
        "PRATKC 合计: 块行={tkc_rows} 展开电路={tkc_flat} 块化例外={tkc_exc} 异常参数={tkc_unknown} 占用={:.2} MB（含索引/版本链）",
        tkc_bytes as f64 / 1048576.0
    );

    println!("\n---- 每个 NeStore 的 N7TKC（稀疏 Circuit + N7TG 句柄，D53 步骤 2）----");
    let (mut n7_spans, mut n7_circ, mut n7_no_handle, mut n7_bytes, mut n7_odd) =
        (0usize, 0usize, 0u64, 0usize, 0u64);
    let (mut n7_arena, mut n7_reused) = (0usize, 0u64);
    for ne in &lib.nes {
        let name = ne
            .name
            .map(|s| String::from_utf8_lossy(lib.interner.resolve(s)).into_owned())
            .unwrap_or_else(|| format!("<meid={} 无名>", ne.meid));
        let c = ne.n7.counts();
        n7_spans += ne.n7.live_spans().len();
        n7_circ += ne.n7.expanded_len();
        n7_arena += ne.n7.spans().len();
        n7_reused += c.reused;
        n7_no_handle += c.no_handle;
        n7_odd += c.cs_odd + c.extra_odd + c.overflow;
        n7_bytes += ne.n7.bytes();
        if ne.n7.expanded_len() == 0 {
            continue;
        }
        println!(
            "{name:8} 段={:3} 电路={:5} 无句柄段={:3} 异常={:3} | {:6.1} KB",
            ne.n7.live_spans().len(),
            ne.n7.expanded_len(),
            c.no_handle,
            c.cs_odd + c.extra_odd + c.overflow,
            ne.n7.bytes() as f64 / 1024.0
        );
    }
    println!(
        "N7TKC 合计: 当前段={n7_spans} 电路={n7_circ}（历史段={n7_arena}，内容寻址复用={n7_reused} 次）无句柄段={n7_no_handle} 异常={n7_odd} 占用={:.2} MB",
        n7_bytes as f64 / 1048576.0
    );

    println!("\n---- 每个 NeStore 的 ASBR（扁平 did + 6 子字段）----");
    let pdraw = lib.pools.register("ASBR.DID");
    let mut asbr_rows = 0usize;
    let mut asbr_alive = 0usize;
    for ne in &lib.nes {
        let name = ne
            .name
            .map(|s| lib.interner.decode(s))
            .unwrap_or_else(|| format!("<meid={} 无名>", ne.meid));
        asbr_rows += ne.asbr.len();
        asbr_alive += ne.asbr.alive_count();
        if ne.asbr.is_empty() {
            continue;
        }
        let st = ne.asbr.stats();
        // DID 分布（与 ims_mem 的输出可直接对）
        let mut dist: std::collections::BTreeMap<String, usize> = Default::default();
        for i in 0..ne.asbr.len() as u32 {
            if !ne.asbr.is_alive(i) {
                continue;
            }
            let Some(row) = ne.asbr.row(i) else { continue };
            let w = row
                .did()
                .and_then(|l| lib.pools.label_sym(pdraw, l))
                .map(|s| lib.interner.decode(s))
                .unwrap_or_else(|| "-".into());
            *dist.entry(w).or_default() += 1;
        }
        println!(
            "{name:8} rows={:6} alive={:6} | add={:6} dup={:4} mod={:3} rmv={:3} key_missing={:3} | DID {dist:?}",
            ne.asbr.len(),
            ne.asbr.alive_count(),
            st.adds,
            st.dup_add,
            st.mods,
            st.rmvs,
            st.key_missing,
        );
    }
    let (mut pri_ok, mut pri_dn) = (0u64, 0usize); for ne in &lib.nes { pri_ok += ne.asbr.pri_ok; pri_dn += ne.asbr.pri_diffs.len(); } println!("ASBR 合计: rows={asbr_rows} alive={asbr_alive} pri_ok={pri_ok} pri_diffs={pri_dn}");
    // ---- 提升列（P1b）：未声明参数的"表级缺省 + 例外" ----
    let (mut n_tables, mut n_params, mut n_const, mut n_ab, mut n_dev, mut n_drop) = (0, 0, 0, 0, 0, 0u64);
    let mut n_unobserved = 0usize;
    let mut n_fallback = 0u64;
    let mut n_enum_fb = 0u64;
    for ne in &lib.nes {
        for t in ne.tables() {
            let (p, c, a, d, dr) = t.lifted_stats();
            n_unobserved += t.lifted_unobserved();
            n_fallback += t.fallbacks();
            n_enum_fb += t.enum_fallbacks();
            if p > 0 {
                n_tables += 1;
                n_params += p;
                n_const += c;
                n_ab += a;
                n_dev += d;
                n_drop += dr;
            }
        }
    }
    println!(
        "\n提升列：{n_tables} 张表 / 未声明参数 {n_params} 个（常量 {} = {:.1}% / 放弃 {}）/ 保留偏离行 {n_dev} / 放弃时丢弃 {n_drop}",
        n_const,
        n_const as f64 * 100.0 / (n_params.max(1)) as f64,
        n_ab
    );
    println!(
        "  预置清单 {} 条（实测没出现 {} 条 —— 都是清单里有、本批数据里没这个参数的）",
        preset_n, n_unobserved
    );
    // 兜底构成（实测）：Bits 稀疏写法 74 + ASBR.EID 域名形态 4960 + MGW.EID 坏形状 6 = 5040
    println!("A2 宽值：原文兜底 {n_fallback} 次（Bits 74 + ASBR.EID 4960 + MGW.EID 6）/ 枚举池溢出 {n_enum_fb} 次");
    {
        // A2/A3 的池规模（A3 池优化后池是 **Library 级共享**的，不能按表求和）
        let mut num_cells = 0usize;
        for ne in &lib.nes {
            for t in ne.tables() {
                num_cells += t.pfx_cells();
            }
        }
        println!(
            "  池（全局共享）：宽值 {} 个 / 字冠数字 {} 个（{:.2} MB）/ 伴生列 {num_cells} 个单元格",
            lib.wide_pool_len(),
            lib.pfx_pool_len(),
            lib.pool_bytes() as f64 / 1048576.0
        );
    }
    {
        let mut per: Vec<(u64, String)> = Vec::new();
        for ne in &lib.nes {
            let nm = ne.name.map(|s| lib.interner.decode(s)).unwrap_or_default();
            for t in ne.tables() {
                if t.fallbacks() > 0 {
                    per.push((t.fallbacks(), format!("{nm}.{}", t.moc())));
                }
            }
        }
        per.sort_by_key(|p| std::cmp::Reverse(p.0));
        for (n, who) in per.iter().take(8) {
            println!("    兜底 {n:4}  {who}");
        }
    }
    if let Some(ne) = lib.ne_by_name(b"AGCF63") {
        let (p, c, a, d, dr) = ne.asbr.lifted().stats();
        println!("  AGCF63.ASBR 提升：参数 {p}（常量 {c} / 放弃 {a}）/ 偏离行 {d} / 丢弃 {dr}");
    }

    let (rss, peak) = proc_mem_kb();
    println!("\n进程内存: 当前 RSS {} KB / 峰值 {} KB（{:.1} MB）", rss, peak, peak as f64 / 1024.0);
}

// ---------- 进程内存（Windows：K32GetProcessMemoryInfo） ----------

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

/// `(当前 RSS, 峰值 RSS)`，单位 KB。失败时返回 `(0, 0)`。
fn proc_mem_kb() -> (usize, usize) {
    let mut c = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: 传的是本进程句柄 + 合法的结构体指针 + 正确长度
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    if ok == 0 {
        (0, 0)
    } else {
        (c.working_set_size / 1024, c.peak_working_set_size / 1024)
    }
}

