//! MGW 的 SQLite 只读投影：43 张表 → `mgw_<moc 小写>`。
//!
//! **命名空间**：一律 `mgw_` 前缀。原因不是"好看"——AGCF 的 149 张表里有 **7 个 MOC 与 MGW 同名**
//! （`BRD` / `IPADDR` / `N7DSP` / `N7LKS` / `N7LNK` / `N7RT` / `OFI`），
//! 而 `proj.db` 里已有的 `cur_BRD` 是 **AGCF 的板卡声明**，不是 MGW 的机框板卡（408 行）：
//! 同名、不同系统、不同列集。用 `mgw_` 前缀后两边不可能撞。
//!
//! 表结构：`(ne TEXT, row INTEGER, <该 MOC 的列…>)`
//!
//! * `ne` 是 MGW 简称（`lh1`…，来自 `mgw_ne_map.tsv`）；**不用 `MID(IP:port)` 当键**，
//!   因为人是按名字查的，IP 是属性（要 IP 就查 `Mgw` 对象或 `mgw_vmgw` 的 `MID` 列）。
//! * `row` 是内存里的行号（**稳定 id**，与 AGCF 投影同一个口径），用于和 hist 视图对齐。
//! * 值全是 TEXT（MGW 侧没有数值语义，原样保留，唯一例外是位图参数已被转成十进制掩码串）。
//! * 只投影**存活行**（`is_alive`）；历史请看内存的列级版本链（未导出 `_hist`，见 README 已知缺口）。

use rusqlite::{params_from_iter, Connection, ToSql};

use ims_mem2::mgw::MgwLibrary;

/// 投影规模。
#[derive(Debug, Default, Clone, Copy)]
pub struct MgwProjStats {
    pub tables: usize,
    pub rows: u64,
    pub cols: usize,
}

/// 把 MGW 的 43 张表写进 `path`（会先 drop 同前缀的旧表，可重复执行）。
pub fn export_mgw(lib: &MgwLibrary, path: &str) -> rusqlite::Result<MgwProjStats> {
    let mut con = Connection::open(path)?;
    con.execute_batch(
        "PRAGMA journal_mode=OFF;
         PRAGMA synchronous=OFF;
         PRAGMA temp_store=MEMORY;",
    )?;
    drop_mgw_objects(&con)?;

    let mut st = MgwProjStats::default();
    let tx = con.transaction()?;
    {
        for (mi, moc) in MgwLibrary::mocs().iter().enumerate() {
            // 有没有网元建过这张表
            let Some(sample) = lib.nes.iter().find_map(|n| n.tables.get(&mi)) else {
                continue;
            };
            let (names, _kinds) = sample.col_meta();
            let tname = format!("mgw_{}", moc.to_ascii_lowercase());
            // DDL
            let mut ddl = format!("CREATE TABLE \"{tname}\" (ne TEXT NOT NULL, row INTEGER NOT NULL");
            for n in names {
                ddl.push_str(&format!(", \"{}\" TEXT", n.to_ascii_lowercase()));
            }
            ddl.push_str(", PRIMARY KEY(ne, row))");
            tx.execute_batch(&ddl)?;
            // 大表建 `(ne)` 索引（体积/收益见 D68 的口径：只给 >=1,000 行的）
            let mut placeholders = String::new();
            for i in 0..names.len() {
                if i > 0 {
                    placeholders.push(',');
                }
                placeholders.push('?');
            }
            let ins = format!(
                "INSERT INTO \"{tname}\" VALUES (?,?{}{})",
                if names.is_empty() { "" } else { "," },
                placeholders
            );
            let mut stmt = tx.prepare(&ins)?;
            let mut local_rows = 0u64;
            for ne in &lib.nes {
                let Some(t) = ne.tables.get(&mi) else { continue };
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) {
                        continue;
                    }
                    let mut vals: Vec<Box<dyn ToSql>> = Vec::with_capacity(names.len() + 2);
                    vals.push(Box::new(ne.short.clone()));
                    vals.push(Box::new(row as i64));
                    for ci in 0..names.len() as u16 {
                        match t.col_sym(row, ci) {
                            Some(s) => {
                                let v = String::from_utf8_lossy(lib.interner.resolve(s)).into_owned();
                                vals.push(Box::new(v));
                            }
                            None => vals.push(Box::new(None::<String>)),
                        }
                    }
                    stmt.execute(params_from_iter(vals.iter().map(|b| b.as_ref())))?;
                    st.rows += 1;
                    local_rows += 1;
                }
            }
            drop(stmt);
            // 行数够多才建 `(ne)` 索引（口径同 D68：INDEX_MIN_ROWS）
            // ⚠ 必须用**本表**行数，不是 st.rows（累计）—— 否则累计过 1,000 后每张表都建索引
            if local_rows >= crate::INDEX_MIN_ROWS {
                tx.execute_batch(&format!(
                    "CREATE INDEX IF NOT EXISTS \"ix_{tname}_ne\" ON \"{tname}\"(ne)"
                ))?;
            }
            st.tables += 1;
            st.cols += names.len();
        }
    }
    tx.commit()?;
    Ok(st)
}

/// 删掉本模块管理的全部对象（`mgw_` 前缀的表 + 索引）。
pub fn drop_mgw_objects(con: &Connection) -> rusqlite::Result<()> {
    let names: Vec<String> = {
        let mut st = con.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'mgw\\_%' ESCAPE '\\'",
        )?;
        let mut rows = st.query([])?;
        let mut v = Vec::new();
        while let Some(r) = rows.next()? {
            v.push(r.get::<_, String>(0)?);
        }
        v
    };
    for n in names {
        con.execute_batch(&format!("DROP TABLE IF EXISTS \"{n}\";"))?;
    }
    Ok(())
}

/// 逐表 `(表名, 行数)`，便于和内存/金标准对账。
pub fn mgw_proj_counts(con: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut st = con.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'mgw\\_%' ESCAPE '\\' ORDER BY name",
    )?;
    let mut rows = st.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let n: String = r.get(0)?;
        let c: i64 = con.query_row(&format!("SELECT COUNT(*) FROM \"{n}\""), [], |r| r.get(0))?;
        out.push((n, c));
    }
    Ok(out)
}
