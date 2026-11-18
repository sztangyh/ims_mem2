//! 导出 SQLite 只读投影，并**逐表哈希自校验**（P5a）。
//!
//! ```powershell
//! cargo run -p ims_mem2_proj --release --offline --example export -- `
//!   ..\agcf_data\ALLME_*.txt
//! ```

use ims_mem2::Library;
use ims_mem2_proj::{db_hashes, export_cur, mem_hashes, schema_of};

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: export <文件1> [文件2 ...]");
        std::process::exit(2);
    }
    let mut db = "temp/proj.db".to_string();
    let _ = std::fs::create_dir_all("temp");
    if std::fs::remove_file(&db).is_err() && std::path::Path::new(&db).exists() {
        // 被别的进程占着（Windows 上 SQLite deny delete）：换一个唯一名字。
        // 否则会在旧库上撞 "already exists"，甚至读到上一次被中断写坏的镜像。
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        db = format!("temp/proj_{stamp}.db");
        println!("WARN: temp/proj.db 被占用，改用 {db}");
    }
    println!("cur_db: {db}");
    // 让检查脚本跟随实际路径（`temp/proj.db` 被占用时会换名）
    let _ = std::fs::write("temp/cur_db.txt", &db);

    let t0 = std::time::Instant::now();
    let mut lib = Library::new();
    if let Ok(b) = std::fs::read("zh.tsv") {
        let n = lib.load_zh(&b);
        println!("zh: {n} 条 / {:.1} MB（zh.tsv）", lib.zh_bytes() as f64 / 1048576.0);
    }
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();
    let t_imp = t0.elapsed();

    let mut tables = 0usize;
    let mut in_mem_rows = 0u64;
    for ne in &lib.nes {
        for t in ne.tables() {
            tables += 1;
            in_mem_rows += t.alive_count() as u64;
        }
    }

    let t1 = std::time::Instant::now();
    let stats = export_cur(&lib, &db).expect("导出失败");
    let t_exp = t1.elapsed();

    let mem = mem_hashes(&lib);
    let schema = schema_of(&lib);
    let t2 = std::time::Instant::now();
    let dbh = db_hashes(&db, &schema).expect("回读失败");
    let t_rd = t2.elapsed();

    if std::env::var("PROJ_DEBUG").is_ok() {
        // 打印 RT 的第一行两侧规范串
        if let Some(ne) = lib.ne_by_name(b"AGCF63") {
            if let Some(t) = ne.table_by_name("RT") {
                let cols = ims_mem2_proj::expand_cols(t);
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) { continue }
                    let mut s = String::from("AGCF63|");
                    for c in cols.iter() {
                        ims_mem2_proj::cell_out(&lib, "RT", t, row, c, None).write(&mut s);
                    }
                    println!("MEM {s}");
                    break;
                }
                let (nm, kd) = t.col_meta();
                println!("COLS n={} k={} expand={}", nm.len(), kd.len(), cols.len());
            }
        }
    }
    // 调试：把 RT 两侧规范串写到文件，便于逐行 diff
    if std::env::var("PROJ_DUMP").is_ok() {
        let mut out = String::new();
        if let Some(ne) = lib.ne_by_name(b"AGCF63") {
            if let Some(t) = ne.table_by_name("RT") {
                let cols = ims_mem2_proj::expand_cols(t);
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) { continue }
                    let mut s = format!("{row}|");
                    for c in cols.iter() {
                        ims_mem2_proj::cell_out(&lib, "RT", t, row, c, None).write(&mut s);
                    }
                    out.push_str(&s);
                    out.push('\n');
                }
            }
        }
        let _ = std::fs::write("temp/rt_mem.txt", out);
    }

    // 调试：按 (MOC, NE) 分别比，找出差异行
    if std::env::var("PROJ_PER_NE").is_ok() {
        for moc in ["RT", "DNSRR"] {
            for (ni, ne) in lib.nes.iter().enumerate() {
                let Some(t) = ne.table_by_name(moc) else { continue };
                let nm = ims_mem2_proj::ne_name(&lib, ne);
                let cols = ims_mem2_proj::expand_cols(t);
                let mut mem_rows: Vec<String> = Vec::new();
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) { continue }
                    let mut s = format!("{row}|");
                    for c in cols.iter() {
                        ims_mem2_proj::cell_out(&lib, moc, t, row, c, None).write(&mut s);
                    }
                    mem_rows.push(s);
                }
                // DB 侧
                let mut db_rows: Vec<String> = Vec::new();
                {
                    let conn = rusqlite::Connection::open(&db).unwrap();
                    let sel = format!(
                        "SELECT \"row\",{} FROM \"cur_{moc}\" WHERE \"ne\"=?",
                        cols.iter().map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(",")
                    );
                    let mut stmt = conn.prepare(&sel).unwrap();
                    let it = stmt.query_map([nm.as_str()], |r| {
                        let row: i64 = r.get(0)?;
                        let mut s = format!("{row}|");
                        for i in 0..cols.len() {
                            let v: rusqlite::types::Value = r.get(i + 1)?;
                            let cell = match v {
                                rusqlite::types::Value::Null => ims_mem2_proj::Cell::Null,
                                rusqlite::types::Value::Integer(x) => ims_mem2_proj::Cell::Int(x),
                                rusqlite::types::Value::Text(t) => ims_mem2_proj::Cell::Text(t),
                                _ => ims_mem2_proj::Cell::Null,
                            };
                            cell.write(&mut s);
                        }
                        Ok(s)
                    }).unwrap();
                    for r in it { db_rows.push(r.unwrap()); }
                }
                mem_rows.sort();
                db_rows.sort();
                let h1 = ims_mem2_proj::fnv1a64(mem_rows.join("\n").as_bytes());
                let h2 = ims_mem2_proj::fnv1a64(db_rows.join("\n").as_bytes());
                let status = if h1 == h2 { "ok" } else { "DIFF" };
                println!("{moc:6} {nm:12} mem={:5} db={:5} {status}", mem_rows.len(), db_rows.len());
                if h1 != h2 {
                    for (a, b) in mem_rows.iter().zip(db_rows.iter()) {
                        if a != b {
                            println!("   mem: {a}");
                            println!("   db : {b}");
                            break;
                        }
                    }
                }
                let _ = ni;
            }
        }
    }

    let mut bad = 0usize;
    for (moc, (n, h)) in &mem {
        match dbh.get(moc) {
            Some((dn, dh)) if dn == n && dh == h => {}
            other => {
                bad += 1;
                if bad <= 5 {
                    println!("  x {moc}: 内存 ({n}, {h:016x}) ≠ DB {other:?}");
                }
            }
        }
    }

    println!(
        "表 {tables} 张 / 内存存活 {in_mem_rows} 行 / 导出 {} 行（{} 张表）/ 索引 {} 个 / zh {} 行",
        stats.rows, stats.tables, stats.indexes, stats.zh_rows
    );
    println!(
        "耗时：导入 {:.2}s / 导出 {:.2}s / 回读校验 {:.2}s",
        t_imp.as_secs_f64(),
        t_exp.as_secs_f64(),
        t_rd.as_secs_f64()
    );
    let size = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
    println!("DB 文件 {:.1} MB（{db}）", size as f64 / 1024.0 / 1024.0);
    if bad == 0 && stats.rows == in_mem_rows {
        println!("ok {bad} 差异：{} 张表逐表哈希一致（内存 == SQLite）", mem.len());
    } else {
        println!("x {bad} 张表哈希不一致；行数 {} vs {}", stats.rows, in_mem_rows);
        std::process::exit(1);
    }
}
