//! SCD2 双批验证：2023（A51/A52）+ 2026（ALLME）→ 同一 MENAME 两个批次。
//!
//! 对账基准来自 `ims_core/docs/M5-STATUS.md` 的实测表（`cnacld`，2023→2026）：
//!
//! | 网元 | 2023 | 2026 | 删除(hist) | 值变更(hist) | 新增(仅 cur) | 净增 |
//! | --- | --- | --- | --- | --- | --- | --- |
//! | AGCF51 | 16690 | 17800 | 2754 | 15 | 3864 | +1110 |
//! | AGCF52 | 16693 | 17827 | 2754 | 12 | 3888 | +1134 |
//!
//! 我们的口径与之对应：`vhist` 里"整行删除版"= 删除、"列级差量版"= 值变更；
//! 首版/新增从不进版本链。所以 AGCF51 的 `cnacld` 版本数应 ≈ 2754 + 15 = 2769。
//!
//! ```powershell
//! cargo run --release --offline --example scd2_check -- `
//!   C:\dev\ims_ws\data\agcf\20230919\A51_20230919.txt `
//!   C:\dev\ims_ws\data\agcf\20230919\A52_20230919.txt `
//!   ..\agcf_data\ALLME_20260921033056.txt ..\agcf_data\ALLME_202609210330361.txt
//! ```

use ims_mem2::Library;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.len() < 3 {
        eprintln!("用法: scd2_check <2023 批次文件...> <2026 批次文件...>");
        std::process::exit(2);
    }

    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
        println!(
            "导入 {p}\n  batch={:?}  AGCF51.cnacld alive={:?}  AGCF52.cnacld alive={:?}",
            lib.batch_no,
            lib.ne_by_name(b"AGCF51").map(|n| n.cnacld.alive_count()),
            lib.ne_by_name(b"AGCF52").map(|n| n.cnacld.alive_count()),
        );
    }
    lib.finish();

    println!("\n===== 双批 SCD2 结果 =====");
    if std::env::var("SCD2_DEBUG").is_ok() {
        if let Some(ne) = lib.ne_by_name(b"AGCF51") {
            use ims_mem2::ColStore;
            let t = &ne.cnacld;
            let mut shown = 0;
            for i in 0..t.len() as u32 {
                if !t.is_alive(i) || t.vhist().row_versions(i).next().is_none() {
                    continue;
                }
                let Some(cur) = t.row(i) else { continue };
                let old = ims_mem2::as_of(cur, t.vhist(), i, 12473490);
                let mut cols = Vec::new();
                for c in 0..<ims_mem2::schema::cnacld_gen::CnacldRow as ColStore>::COLS {
                    if old.get_col(c) != cur.get_col(c) {
                        cols.push(c);
                    }
                }
                if cols.is_empty() {
                    if let Some(v) = t.vhist().row_versions(i).next() {
                        println!(
                            "  行 {i}: 变化列 [] ← 版本 cols={:?} vals={:?} valid=[{}, {})",
                            v.cols, v.vals, v.valid_from, v.valid_to
                        );
                    }
                } else {
                    println!("  行 {i}: 变化列 {cols:?}");
                }
                shown += 1;
                if shown >= 5 {
                    break;
                }
            }
        }
    }
    for name in [b"AGCF51", b"AGCF52"] {
        let Some(ne) = lib.ne_by_name(name) else { continue };
        let t = &ne.cnacld;
        let st = t.stats();
        // (a) 起：删行不再写列级版本，只记 `dead_at`（投影成 hist_row）。
        // 所以 vhist.len() **就是**"值变更的列级版本数"，删除数单独由计数器给出。
        let deleted = st.snapshot_deletes + st.rmv_applied;
        println!(
            "{}: cur={}  值变更版本={}  删除={}（快照 {} + 显式 RMV {}）  batch={}",
            String::from_utf8_lossy(name),
            t.alive_count(),
            t.vhist().len(),
            deleted,
            st.snapshot_deletes,
            st.rmv_applied,
            ne.batch,
        );

        println!(
            "  diff(2023→2026): {d:?}",
            d = t.diff(12473490, 14054610)
        );

        // 历史回查：找一个"在 cur 里已不存在"的键
        let mut found = 0;
        for i in 0..t.len() as u32 {
            if t.is_alive(i) {
                continue;
            }
            // 从版本链里反查该行的键不值当，这里只做"能查到版本"的抽样
            found += 1;
            if found > 2 {
                break;
            }
        }
        println!("  （被删行样本 {found} 条；版本链非空 = {}）", !t.vhist().is_empty());
    }

    // ---- ASBR 双批（D59 / seen bug 的回归挡板）----
    //
    // 为什么专门查 ASBR：它的 `SRVRGT` 合并在批末写回 `SN`，那些行可能本批没出现；
    // 早期用 `set_cell` 写回时把 `seen` 标上了 → 快照删除漏删 127,964 行。
    // 基准来自原始 MML 独立统计（2023 = A51/A52，2026 = 033056/0330361）：
    //   AGCF51: 2023 265,817 / 2026 167,536 → 应删 135,704
    //   AGCF52: 2023 262,778 / 2026 133,020 → 应删 133,349
    for (name, want_cur, want_del) in [
        (b"AGCF51", 167_536u64, 135_704u64),
        (b"AGCF52", 133_020u64, 133_349u64),
    ] {
        let Some(ne) = lib.ne_by_name(name) else { continue };
        let st = ne.asbr.core.stats();
        let cur = ne.asbr.core.alive_count() as u64;
        let ok = cur == want_cur && st.snapshot_deletes == want_del;
        println!(
            "ASBR {}: cur={}（期望 {want_cur}） 快照删除={}（期望 {want_del}） {}",
            String::from_utf8_lossy(name),
            cur,
            st.snapshot_deletes,
            if ok { "ok" } else { "MISMATCH" }
        );
    }

    let (a, b) = (lib.ne_by_name(b"AGCF51"), lib.ne_by_name(b"AGCF52"));
    if let (Some(a), Some(b)) = (a, b) {
        println!(
            "\n期望（ims_core M5-STATUS）：AGCF51 17800 / 删除 2754 / 值变更 15；AGCF52 17827 / 2754 / 12"
        );
        println!(
            "实测：AGCF51 {} / {} / {}；AGCF52 {} / {} / {}",
            a.cnacld.alive_count(),
            a.cnacld.stats().snapshot_deletes + a.cnacld.stats().rmv_applied,
            a.cnacld.vhist().len(),
            b.cnacld.alive_count(),
            b.cnacld.stats().snapshot_deletes + b.cnacld.stats().rmv_applied,
            b.cnacld.vhist().len(),
        );
    }
}
