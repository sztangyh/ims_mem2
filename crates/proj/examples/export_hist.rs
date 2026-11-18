//! 历史投影 + 回放自校验（P5b）。
//!
//! 1. 版本链导出成 `hist_<MOC>`（`ne/row/key/col/old/valid_from/valid_to`）；
//! 2. **只用 SQLite（`cur_` + `hist_`）回放某行在 2023 批的状态**，
//!    与内存 `at(key, 2023)` 逐列比对——投影能自己讲清历史，才算完整。
//!
//! ```powershell
//! cargo run -p ims_mem2_proj --release --offline --example export_hist -- `
//!   C:\dev\ims_ws\data\agcf\20230919\A51_20230919.txt `
//!   C:\dev\ims_ws\data\agcf\20230919\A52_20230919.txt `
//!   ..\agcf_data\ALLME_20260921033056.txt ..\agcf_data\ALLME_202609210330361.txt
//! ```

use std::collections::HashMap;

use ims_mem2::{as_of, Library, TableApi};
use ims_mem2_proj::{
    create_hist_ver_view, export_cur, export_hist, export_hist_diff, export_hist_row, fnv1a64,
    row_cell_in, Cell,
};
use rusqlite::Connection;

const BATCH_2023: i64 = 12473490;
const BATCH_2026: i64 = 14054610;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.len() < 3 {
        eprintln!("用法: export_hist <2023 文件...> <2026 文件...>");
        std::process::exit(2);
    }
    let _ = std::fs::create_dir_all("temp");
    let mut db = "temp/proj_hist.db".to_string();
    if std::fs::remove_file(&db).is_err() && std::path::Path::new(&db).exists() {
        // 被别的进程占着（Windows 上 SQLite 默认 deny delete）：换一个唯一名字，
        // 否则后面的 CREATE TABLE 会撞 "already exists" 直接 panic。
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        db = format!("temp/proj_hist_{stamp}.db");
        println!("WARN: temp/proj_hist.db 被占用，改用 {db}");
    }
    println!("hist_db: {db}");

    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    let t0 = std::time::Instant::now();
    let cur_stats = export_cur(&lib, &db).expect("导出 cur 失败");
    let mut conn = Connection::open(&db).expect("打开 DB");
    conn.execute_batch("PRAGMA temp_store=MEMORY;").expect("temp_store");
    let hist_records = export_hist(&lib, &mut conn).expect("导出 hist 失败");
    let dt = t0.elapsed().as_secs_f64();

    let hist_tables: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'hist_%'")
            .unwrap();
        let it = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        it.map(|x| x.unwrap()).collect()
    };
    let mut hist_versions = 0i64;
    for t in &hist_tables {
        let q = format!(
            "SELECT COUNT(*) FROM (SELECT DISTINCT \"ne\",\"row\",\"valid_from\",\"valid_to\" FROM \"{t}\")"
        );
        hist_versions += conn.query_row(&q, [], |r| r.get::<_, i64>(0)).unwrap_or_else(|e| panic!("表 {t}: {e}"));
    }

    let diff_rows = export_hist_diff(&lib, &mut conn).expect("导出 hist_diff 失败");
    let row_rows = export_hist_row(&lib, &mut conn).expect("导出 hist_row 失败");
    println!("hist_diff：{diff_rows} 条 (行, 列, 版本) 的 old→new 明细");
    println!("hist_row：{row_rows} 条（每行一条生命周期 + 死行镜像）");
    create_hist_ver_view(&conn).expect("建 hist_ver 视图失败");
    let ver_n: i64 = conn
        .query_row("SELECT COUNT(*) FROM hist_ver", [], |r| r.get(0))
        .unwrap_or(0);
    println!("hist_ver：{ver_n} 条（每次版本一条，把改动列聚合成 列=旧→新）");
    let j_n: i64 = conn
        .query_row("SELECT COUNT(*) FROM hist_ver_json", [], |r| r.get(0))
        .unwrap_or(0);
    println!("hist_ver_json：{j_n} 条（同上的 JSON 形态）");
    println!(
        "cur: {} 表 / {} 行；hist: {} 表 / {} 条 (版本,列) / {} 条版本；导出 {:.2}s",
        cur_stats.tables,
        cur_stats.rows,
        hist_tables.len(),
        hist_records,
        hist_versions,
        dt
    );

    for ne_name in ["AGCF51", "AGCF52"] {
        let Some(ne) = lib.ne_by_name(ne_name.as_bytes()) else {
            continue;
        };
        let t = &ne.cnacld;
        let (names, kinds) = t.col_meta();

        // hist：键 → [(列, 旧值, valid_from, valid_to)]
        // (列名, 旧值, valid_from, valid_to)
        type HistRec = (String, Option<String>, i64, i64);
        let mut hmap: HashMap<i64, Vec<HistRec>> = HashMap::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT \"row\",\"col\",\"old\",\"valid_from\",\"valid_to\" FROM hist_cnacld WHERE \"ne\"=?",
                )
                .unwrap();
            let it = stmt
                .query_map([ne_name], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                })
                .unwrap();
            for row in it {
                let (row_id, col, old, vf, vt) = row.unwrap();
                hmap.entry(row_id).or_default().push((col, old, vf, vt));
            }
        }

        let sel = format!(
            "SELECT \"row\",{} FROM cur_cnacld WHERE \"ne\"=?",
            names
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut stmt = conn.prepare(&sel).unwrap();
        let it = stmt
            .query_map([ne_name], |r| {
                let row_id: i64 = r.get(0)?;
                let mut vals: Vec<Option<String>> = Vec::new();
                for i in 0..names.len() {
                    let v: rusqlite::types::Value = r.get(i + 1)?;
                    vals.push(match v {
                        rusqlite::types::Value::Null => None,
                        rusqlite::types::Value::Integer(i) => Some(i.to_string()),
                        rusqlite::types::Value::Text(s) => Some(s),
                        _ => None,
                    });
                }
                Ok((row_id, vals))
            })
            .unwrap();

        let mut checked = 0usize;
        let mut bad = 0usize;
        let mut mem_rows: Vec<String> = Vec::new();
        let mut db_rows: Vec<String> = Vec::new();
        for row in it {
            let (row_id, cur_vals) = row.unwrap();
            let Some(cur_row) = t.row(row_id as u32) else {
                continue;
            };
            let mem = as_of(cur_row, t.vhist(), row_id as u32, BATCH_2023);
            if !t.exists_at(row_id as u32, BATCH_2023) {
                continue; // 2023 时还不存在
            }
            // DB 侧：cur + 回退 valid_to > 2023 的历史列（按 valid_from 降序）
            let mut st = cur_vals.clone();
            if let Some(hs) = hmap.get(&row_id) {
                let mut hs = hs.clone();
                hs.sort_by_key(|(_, _, vf, _)| std::cmp::Reverse(*vf));
                for (col, old, vf, vt) in hs {
                    if vt > BATCH_2023 && vf < BATCH_2026 {
                        if let Some(pos) = names.iter().position(|n| *n == col) {
                            st[pos] = old;
                        }
                    }
                }
            }
            let mut mem_s = String::new();
            let mut ok = true;
            for (c, name) in names.iter().enumerate() {
                let cell = row_cell_in(t, &lib, t.moc(), &mem, c as u16, kinds[c], name);
                let v = match cell {
                    Cell::Null => None,
                    Cell::Int(x) => Some(x.to_string()),
                    Cell::Text(s) => Some(s),
                };
                if v != st[c] {
                    ok = false;
                    if bad < 3 {
                        println!("  x {ne_name} 行{row_id} 列 {name}: 内存 {:?} ≠ DB {:?}", v, st[c]);
                    }
                }
                mem_s.push_str(&format!("{}|", v.unwrap_or_default()));
            }
            mem_rows.push(format!("{row_id}|{mem_s}"));
            let mut ds = format!("{row_id}|");
            for v in &st {
                ds.push_str(&format!("{}|", v.clone().unwrap_or_default()));
            }
            db_rows.push(ds);
            checked += 1;
            if !ok {
                bad += 1;
            }
        }
        mem_rows.sort();
        db_rows.sort();
        let h1 = fnv1a64(mem_rows.join("\n").as_bytes());
        let h2 = fnv1a64(db_rows.join("\n").as_bytes());
        println!(
            "  {ne_name} cnacld @2023：核对 {checked} 行 / 不一致 {bad} 行；内存哈希 {h1:016x} / DB 回放哈希 {h2:016x}"
        );
        if bad > 0 || h1 != h2 {
            std::process::exit(1);
        }
    }
}




