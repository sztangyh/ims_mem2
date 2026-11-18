//! SQLite **只读投影**（P5a）：内存是唯一真源，SQLite 只用来"给别人查"。
//!
//! 三条规矩：
//! 1. **单向**：只写不读；导入后重算整库，不做双向同步；
//! 2. **不丢列**：`cur_<MOC>` 一表一 MOC，列名 = 参数名，`ne` + `row` 额外带上；
//! 3. **可校验**：每张表都算"规范行串"的 FNV 哈希，与内存侧比对（`verify` 模块）。
//!
//! 文本/枚举导出成原文（从 `Interner` / `LabelCols` 反解），数值/布尔导出成整数，
//! 缺参导出成 `NULL`。

pub mod mgw;

pub use mgw::{drop_mgw_objects, export_mgw, mgw_proj_counts, MgwProjStats};

use ims_mem2::{Library, Sym, TableApi};
use rusqlite::Connection;
use std::collections::BTreeMap;

/// 导出统计。
#[derive(Debug, Default, Clone)]
pub struct ProjStats {
    pub tables: usize,
    pub rows: u64,
    /// 建了多少个索引（`ix_cur_*` / `ix_hist_*`）
    pub indexes: usize,
    /// 中文词典写入行数（`zh_*` 5 张表合计；0 = 未装载 `zh.tsv`）
    pub zh_rows: usize,
}

/// 建索引的行数门槛。
///
/// 实测（`temp/proj.db`，156.5 MB / 943,104 行 / 149 张 `cur_*`）：
///
/// | 方案 | 建索引耗时 | 库体积 | 覆盖行数 |
/// |---|---|---|---|
/// | 全部 149 张 | 4.7 s | +13.9 MB | 100% |
/// | 只建行数前 10 | 1.25 s | +12.8 MB | 95.7% |
///
/// 前 10 张 = ASBR 673,407 / CNACLD 79,669 / CNACLR 29,909 / SRTCOST 29,274 /
/// CLRDSN 26,875 / SPDNC 26,637 / MGW 21,510 / V5TKC 5,518 / TGLDIDX 5,190 / TGSRT 4,916
/// —— 1,000 行这个门槛正好把前 10 张圈进来（第 11 张 N7TKC 4,448 行会被漏掉，
/// 但它的查询本来就少；要更激进把门槛调到 1,000 以下即可）。
pub const INDEX_MIN_ROWS: u64 = 1_000;

/// 一列的渲染值（内存侧 / DB 侧共用，保证两边口径完全一致）。
#[derive(Clone, Debug)]
pub enum Cell {
    Null,
    Int(i64),
    Text(String),
}

impl Cell {
    pub fn write(&self, out: &mut String) {
        match self {
            Cell::Null => out.push('-'),
            Cell::Int(v) => out.push_str(&v.to_string()),
            Cell::Text(s) => out.push_str(s),
        }
        out.push('|');
    }
}

/// FNV-1a 64（与 `examples/*_hash.rs` 同一实现）。
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &c in bytes {
        h ^= c as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 网元名：CGP 段没有 MENAME，用 `CGP` 占位（否则 CGP 段那几十行会被导出漏掉）。
pub fn ne_name(lib: &Library, ne: &ims_mem2::NeStore) -> String {
    match ne.name {
        Some(s) => lib.interner.decode(s),
        None => format!("CGP:MEID{}", ne.meid),
    }
}

fn tag_of(kind: u8) -> &'static str {
    match kind {
        // A2 的宽值列（`Ip/Eid/Bits/Pair/Spc`）在投影里也落成 TEXT（渲染回原文）
        b't' | b'e' | b'i' | b'd' | b'x' | b'p' | b'c' | b'N' => "TEXT",
        b'b' | b'n' => "INTEGER",
        _ => "TEXT",
    }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// 内存里取一行某列的值。
#[allow(clippy::too_many_arguments)] // 导出路径参数多：lib/moc/表/行/列/类型/列名/URI 域
pub fn cell_of(
    lib: &Library,
    moc: &str,
    t: &dyn TableApi,
    row: u32,
    col: u16,
    kind: u8,
    name: &str,
    uri_dom: Option<ims_mem2::Sym>,
) -> Cell {
    match kind {
        // A3：`Pfx` 伴生列（`<NAME>_NUM`）—— 池里是 `PfxNum`，渲染成 `K'` + 大写数字
        b'N' => match t.pfx_text(row, col) {
            Some(s) => Cell::Text(s),
            None => Cell::Null,
        },
        // A2：宽值列（Ip/Eid/Bits/Pair/Spc）—— typed 走宽值池渲染，原文兜底走 `col_sym`
        b'i' | b'd' | b'x' | b'p' | b'c' => match t.wide_text(row, col) {
            Some(s) => Cell::Text(s),
            None => match t.col_sym(row, col) {
                Some(s) => Cell::Text(lib.interner.decode(s)),
                None => Cell::Null,
            },
        },
        b't' => match t.col_sym(row, col) {
            None => Cell::Null,
            Some(s) => {
                let raw = lib.interner.decode(s);
                // P6.1：PUI 可能只存了号码，导出/投影要还原成完整 URI
                Cell::Text(lib.rehydrate_uri(raw, uri_dom))
            }
        },
        b'b' => match t.col_u32(row, col) {
            None => Cell::Null,
            Some(v) => Cell::Int(if v != 0 { 1 } else { 0 }),
        },
        b'e' => match t.col_u32(row, col) {
            None => Cell::Null,
            Some(v) => {
                let pool = lib
                    .pools
                    .find(&format!("{moc}.{name}"))
                    .unwrap_or(usize::MAX);
                match ims_mem2::Label::from_u8_0(v as u8)
                    .and_then(|l| lib.pools.label_sym(pool, l))
                {
                    Some(s) => Cell::Text(lib.interner.decode(s)),
                    None => Cell::Null,
                }
            }
        },
        _ => match t.col_u32(row, col) {
            None => Cell::Null,
            Some(v) => Cell::Int(v as i64),
        },
    }
}

/// 取一个**具体行**（任意 `ColStore` 行）某列的渲染值。
///
/// ⚠ A2/A3 起，宽值列与字冠伴生列的值在**表级池**里，所以历史上只用这个函数
/// 已经不够了 —— 历史回放要用 [`row_cell_in`]（带表）。
pub fn row_cell<R: ims_mem2::ColStore>(
    lib: &Library,
    moc: &str,
    row: &R,
    col: u16,
    kind: u8,
    name: &str,
) -> Cell {
    let v = row.get_col(col);
    match kind {
        b't' => match v.as_sym() {
            None => Cell::Null,
            Some(s) => Cell::Text(lib.interner.decode(s)),
        },
        b'b' => match v.as_u32() {
            None => Cell::Null,
            Some(x) => Cell::Int(if x != 0 { 1 } else { 0 }),
        },
        b'e' => match v.as_u32() {
            None => Cell::Null,
            Some(x) => {
                let pool = lib.pools.find(&format!("{moc}.{name}")).unwrap_or(usize::MAX);
                match ims_mem2::Label::from_u8_0(x as u8).and_then(|l| lib.pools.label_sym(pool, l)) {
                    Some(s) => Cell::Text(lib.interner.decode(s)),
                    None => Cell::Null,
                }
            }
        },
        _ => match v.as_u32() {
            None => Cell::Null,
            Some(x) => Cell::Int(x as i64),
        },
    }
}

/// 带表的行渲染（A2/A3 的池需要表）：历史回放/`as_of` 行用它。
pub fn row_cell_in<R: ims_mem2::ColStore>(
    t: &dyn TableApi,
    lib: &Library,
    moc: &str,
    row: &R,
    col: u16,
    kind: u8,
    name: &str,
) -> Cell {
    let v = row.get_col(col);
    match kind {
        // A3：字冠伴生列 —— 池下标 → `K'` + 数字
        b'N' => v
            .as_group()
            .and_then(|idx| t.pfx_at(idx))
            .map_or(Cell::Null, |n| Cell::Text(ims_mem2::frombytes::render_pfx(n))),
        // A2：宽值列 —— 池下标 → 渲染；原文兜底 → 原文
        b'i' | b'd' | b'x' | b'p' | b'c' => match v.as_group() {
            Some(idx) => t
                .wide_at(idx)
                .and_then(|w| t.render_wide(col, w))
                .map_or(Cell::Null, Cell::Text),
            None => v
                .as_sym()
                .map_or(Cell::Null, |s| Cell::Text(lib.interner.decode(s))),
        },
        _ => row_cell(lib, moc, row, col, kind, name),
    }
}

/// 投影层的一列：普通列或"参数组展开出来的第 i 个元素"。
#[derive(Clone, Debug)]
pub struct OutCol {
    pub name: String,
    pub kind: u8,
    pub col: u16,
    pub elem: Option<u8>,
}

/// 把表声明展开成"投影列"：参数组按宽度展开回 `SR1..SR28`（P4）。
pub fn expand_cols(t: &dyn TableApi) -> Vec<OutCol> {
    let (names, kinds) = t.col_meta();
    let groups = t.col_groups();
    let mut out = Vec::with_capacity(names.len());
    for (c, (n, k)) in names.iter().zip(kinds.iter()).enumerate() {
        let c16 = c as u16;
        match groups.iter().find(|(gc, _)| *gc == c16) {
            Some((_, w)) => {
                for i in 0..*w {
                    out.push(OutCol {
                        name: format!("{n}{}", i as u32 + 1),
                        kind: b'n',
                        col: c16,
                        elem: Some(i),
                    });
                }
            }
            None => out.push(OutCol {
                name: n.to_string(),
                kind: *k,
                col: c16,
                elem: None,
            }),
        }
    }
    out
}

/// 取一行某一"投影列"的值。
pub fn cell_out(
    lib: &Library,
    moc: &str,
    t: &dyn TableApi,
    row: u32,
    c: &OutCol,
    uri_dom: Option<ims_mem2::Sym>,
) -> Cell {
    if let Some(i) = c.elem {
        return match t.group_of(row, c.col).and_then(|g| g.get(i as usize)) {
            None => Cell::Null,
            Some(v) => Cell::Int(v as i64),
        };
    }
    cell_of(lib, moc, t, row, c.col, c.kind, &c.name, uri_dom)
}

/// 内存侧的"表 → 规范行串哈希"（`ne|col|col|…`，按行排序后整体哈希）。
pub fn mem_hashes(lib: &Library) -> BTreeMap<String, (u64, u64)> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for ne in &lib.nes {
        let nm = ne_name(lib, ne);
        for t in ne.tables() {
            let moc = t.moc();
            let cols = expand_cols(t);
            let uri_dom = lib.uri_default(
                lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap_or(usize::MAX),
                moc,
            );
            let mut rows = Vec::with_capacity(t.alive_count());
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                let mut s = String::with_capacity(64);
                s.push_str(&nm);
                s.push('|');
                for c in cols.iter() {
                    cell_out(lib, moc, t, row, c, uri_dom).write(&mut s);
                }
                rows.push(s);
            }
            out.entry(moc.to_string()).or_default().extend(rows);
        }
    }
    out.into_iter()
        .map(|(moc, mut rows)| {
            rows.sort();
            let joined = rows.join("\n");
            (moc, (rows.len() as u64, fnv1a64(joined.as_bytes())))
        })
        .collect()
}

/// 幂等：把库里已有的 `cur_*` 表和视图（+ 它们的 `ix_cur_*` 索引）先删掉。
///
/// 为什么需要：`temp/proj.db` 若被别的进程占着，`remove_file` 会失败（Windows 上 SQLite deny delete），
/// 之后 `CREATE TABLE cur_X` 就撞 "already exists" 直接 panic —— 门里表现为"投影自校验失败"。
pub fn drop_cur_objects(conn: &Connection) -> rusqlite::Result<()> {
    drop_objects(
        conn,
        "name LIKE 'cur\\_%' ESCAPE '\\' OR name = 'trunk_cur' \
         OR name LIKE 'ix\\_cur\\_%' ESCAPE '\\'",
    )
}

/// 幂等：把库里已有的 `hist_*` 表与 `hist_row`/`hist_diff`/`hist_ver*` 视图先删掉。
///
/// 和 [`drop_cur_objects`] 同理：复用同一个库文件时不能撞 "already exists"。
pub fn drop_hist_objects(conn: &Connection) -> rusqlite::Result<()> {
    drop_objects(
        conn,
        "name LIKE 'hist\\_%' ESCAPE '\\' \
         OR name IN ('hist_row', 'hist_diff', 'hist_ver', 'hist_ver_json') \
         OR name LIKE 'ix\\_hist\\_%' ESCAPE '\\'",
    )
}

/// 按 `sqlite_master.name` 的匹配条件删对象（表/视图/索引各自用对的 DROP 关键字）。
///
/// 索引要单独处理：`DROP INDEX` 与 `DROP TABLE` 不能混用；而索引名故意**不以** `cur_`/`hist_`
/// 开头（叫 `ix_cur_*`/`ix_hist_*`，一眼能看出是谁的索引），所以必须显式列出来，
/// 否则某张表从 schema 里消失后它的索引会残留，下次 `CREATE INDEX` 撞名。
fn drop_objects(conn: &Connection, where_clause: &str) -> rusqlite::Result<()> {
    let mut names: Vec<(String, String)> = Vec::new();
    {
        let mut st = conn.prepare(&format!(
            "SELECT type, name FROM sqlite_master WHERE {where_clause}"
        ))?;
        let it = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for x in it {
            names.push(x?);
        }
    }
    // 先删索引/视图、后删表：视图可以引用表，索引不能悬空
    names.sort_by_key(|(kind, _)| match kind.as_str() {
        "index" => 0,
        "view" => 1,
        _ => 2,
    });
    for (kind, name) in names {
        let kw = match kind.as_str() {
            "view" => "VIEW",
            "index" => "INDEX",
            _ => "TABLE",
        };
        let ident = name.replace('"', "\"\"");
        conn.execute_batch(&format!("DROP {kw} IF EXISTS \"{ident}\" "))?;
    }
    Ok(())
}

/// 幂等：把库里已有的 `zh_*` 表先删掉。
///
/// 中文词典是**显示用的旁路维度**（与 ims_core 的 5 张 `zh_*` 同名同构），
/// 不参与 `cur_*` 的哈希自校验，所以单独一套 drop。
pub fn drop_zh_objects(conn: &Connection) -> rusqlite::Result<()> {
    drop_objects(conn, "name LIKE 'zh\\_%' ESCAPE '\\'")
}

/// 把中文词典投影成 5 张表（`zh_moc` / `zh_param` / `zh_param_any` / `zh_value` / `zh_value_any`）。
///
/// 名称与列跟 ims_core 的 `zh_*` 对齐，便于两边直接 join 对拍。
/// 返回写入的总行数；词典未装载（`load_zh` 没调）时建空表并返回 0。
///
/// ⚠ **必须在已打开的事务里调用**（[`export_cur`] 就是这么用的）：
/// 1.6 万条逐条 `execute` 若走自动提交，每条一次 fsync —— 实测这里把导出
/// 从 7 s 拖到 **340 s**（踩过）。另外 `journal_mode=OFF` 也是 `export_cur` 设的。
pub fn export_zh(lib: &Library, conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    drop_zh_objects(conn)?;
    let z = lib.zh();
    let mut n = 0usize;
    conn.execute_batch(
        "CREATE TABLE \"zh_moc\" (\"moc\" TEXT NOT NULL, \"op\" TEXT NOT NULL, \
         \"cmd_cn\" TEXT, \"name_cn\" TEXT); \
         CREATE TABLE \"zh_param\" (\"moc\" TEXT NOT NULL, \"param\" TEXT NOT NULL, \
         \"param_cn\" TEXT); \
         CREATE TABLE \"zh_param_any\" (\"param\" TEXT NOT NULL, \"param_cn\" TEXT, \
         \"n_moc\" INTEGER, \"ambiguous\" INTEGER); \
         CREATE TABLE \"zh_value\" (\"moc\" TEXT NOT NULL, \"param\" TEXT NOT NULL, \
         \"value\" TEXT NOT NULL, \"value_cn\" TEXT); \
         CREATE TABLE \"zh_value_any\" (\"param\" TEXT NOT NULL, \"value\" TEXT NOT NULL, \
         \"value_cn\" TEXT, \"n_moc\" INTEGER, \"ambiguous\" INTEGER); \
         CREATE TABLE \"zh_switch\" (\"moc\" TEXT NOT NULL, \"param\" TEXT NOT NULL, \
         \"switch_name\" TEXT NOT NULL, \"means_cn\" TEXT, \"src\" TEXT);",
    )?;
    {
        let mut st = conn.prepare("INSERT INTO \"zh_moc\" VALUES (?,?,?,?)")?;
        for (m, o, a, b) in z.iter_moc() {
            st.execute(rusqlite::params![m, o, a, b])?;
            n += 1;
        }
    }
    {
        let mut st = conn.prepare("INSERT INTO \"zh_param\" VALUES (?,?,?)")?;
        for (m, p, t) in z.iter_param() {
            st.execute(rusqlite::params![m, p, t])?;
            n += 1;
        }
    }
    {
        let mut st = conn.prepare("INSERT INTO \"zh_param_any\" VALUES (?,?,?,?)")?;
        for (p, t, nm, a) in z.iter_param_any() {
            st.execute(rusqlite::params![p, t, nm as i64, a as i64])?;
            n += 1;
        }
    }
    {
        let mut st = conn.prepare("INSERT INTO \"zh_value\" VALUES (?,?,?,?)")?;
        for (m, p, v, t) in z.iter_value() {
            st.execute(rusqlite::params![m, p, v, t])?;
            n += 1;
        }
    }
    {
        let mut st = conn.prepare("INSERT INTO \"zh_value_any\" VALUES (?,?,?,?,?)")?;
        for (p, v, t, nm, a) in z.iter_value_any() {
            st.execute(rusqlite::params![p, v, t, nm as i64, a as i64])?;
            n += 1;
        }
    }
    {
        let mut st = conn.prepare("INSERT INTO \"zh_switch\" VALUES (?,?,?,?,?)")?;
        for (m, p, s, t, src) in z.iter_switch() {
            st.execute(rusqlite::params![m, p, s, t, src])?;
            n += 1;
        }
    }
    // 两级查表都按 (moc, param) / param 走，各建一条；开关名按三元组查
    conn.execute_batch(
        "CREATE INDEX \"ix_zh_param_moc\" ON \"zh_param\" (\"moc\"); \
         CREATE INDEX \"ix_zh_param_any\" ON \"zh_param_any\" (\"param\"); \
         CREATE INDEX \"ix_zh_value_key\" ON \"zh_value\" (\"moc\", \"param\"); \
         CREATE INDEX \"ix_zh_switch_key\" ON \"zh_switch\" (\"moc\", \"param\", \"switch_name\");",
    )?;
    Ok(n)
}

pub fn export_cur(lib: &Library, path: &str) -> rusqlite::Result<ProjStats> {
    let mut conn = Connection::open(path)?;
    drop_cur_objects(&conn)?;
    conn.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA temp_store=MEMORY;")?;

    // 收集 MOC → 展开后的投影列
    let mut schema: BTreeMap<String, Vec<OutCol>> = BTreeMap::new();
    for ne in &lib.nes {
        for t in ne.tables() {
            schema.entry(t.moc().to_string()).or_insert_with(|| expand_cols(t));
        }
    }

    let mut stats = ProjStats::default();
    let tx = conn.transaction()?;
    for (moc, cols) in &schema {
        let tbl = quote_ident(&format!("cur_{moc}"));
        let mut ddl = format!("CREATE TABLE {tbl} (\"ne\" TEXT NOT NULL, \"row\" INTEGER NOT NULL");
        for c in cols.iter() {
            ddl.push_str(&format!(", {} {}", quote_ident(&c.name), tag_of(c.kind)));
        }
        ddl.push_str(");");
        tx.execute_batch(&ddl)?;
        stats.tables += 1;
        let mut table_rows = 0u64;

        // 多值 INSERT：一次提交 N 行（SQLite 默认变量上限 32766，按列数反推 N）
        let vars_per_row = cols.len() + 2;
        // 实测（A/B 同窗口）：多值 INSERT 反而更慢（5.2–5.8s → 6.5–6.6s），
        // 成本在"每格 Value 的构造"而不是语句次数，所以保持一行一执行（D44）。
        let chunk = 1usize;
        let one_row = format!(
            "({})",
            std::iter::repeat_n("?", vars_per_row)
                .collect::<Vec<_>>()
                .join(",")
        );
        let sql = format!(
            "INSERT INTO {tbl} VALUES {}",
            std::iter::repeat_n(one_row.as_str(), chunk)
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut stmt = tx.prepare(&sql)?;
        let mut buf: Vec<rusqlite::types::Value> = Vec::with_capacity(chunk * vars_per_row);
        let mut pending = 0usize;
        for ne in &lib.nes {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let ne_name = ne_name(lib, ne);
            let uri_dom = lib.uri_default(
                lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap_or(usize::MAX),
                moc,
            );
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                let mut vals: Vec<rusqlite::types::Value> = Vec::with_capacity(cols.len() + 2);
                vals.push(rusqlite::types::Value::Text(ne_name.clone()));
                vals.push(rusqlite::types::Value::Integer(i64::from(row)));
                for c in cols.iter() {
                    let cell = cell_out(lib, moc, t, row, c, uri_dom);
                    vals.push(match cell {
                        Cell::Null => rusqlite::types::Value::Null,
                        Cell::Int(v) => rusqlite::types::Value::Integer(v),
                        Cell::Text(s) => rusqlite::types::Value::Text(s),
                    });
                }
                buf.extend(vals);
                pending += 1;
                if pending == chunk {
                    stmt.execute(rusqlite::params_from_iter(buf.iter()))?;
                    buf.clear();
                    pending = 0;
                }
                stats.rows += 1;
                table_rows += 1;
            }
        }
        if pending > 0 {
            // 最后一批不足 chunk：用一条短语句（行数 = pending）
            let short = format!(
                "INSERT INTO {tbl} VALUES {}",
                std::iter::repeat_n(one_row.as_str(), pending)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            tx.prepare_cached(&short)?.execute(rusqlite::params_from_iter(buf.iter()))?;
            buf.clear();
        }
        // 热表建 `(ne)` 索引（见 [`INDEX_MIN_ROWS`]）：投影里几乎所有查询都是 `WHERE ne=?`
        if table_rows >= INDEX_MIN_ROWS {
            tx.execute_batch(&format!(
                "CREATE INDEX {} ON {tbl}(\"ne\")",
                quote_ident(&format!("ix_cur_{moc}_ne"))
            ))?;
            stats.indexes += 1;
        }
    }
    // ---- trunk_cur 视图（P7）：三族中继的公共属性 + 缺省主叫号码 ----
    let mut branches: Vec<String> = Vec::new();
    for fam in ims_mem2::trunk::FAMILIES {
        let Some(cols) = schema.get(fam.moc) else { continue };
        let has = |n: &str| cols.iter().any(|c| c.name.eq_ignore_ascii_case(n));
        if !has("TG") {
            continue;
        }
        let mut sel: Vec<String> = vec![
            "\"ne\"".into(),
            format!("'{}' AS \"kind\"", fam.moc),
        ];
        for c in ims_mem2::trunk::COMMON_COLS {
            if has(c) {
                sel.push(format!("\"{c}\""));
            } else {
                sel.push(format!("NULL AS \"{c}\""));
            }
        }
        if has(fam.dn_param) {
            sel.push(format!("\"{}\" AS \"DN\"", fam.dn_param));
        } else {
            sel.push("NULL AS \"DN\"".into());
        }
        branches.push(format!(
            "SELECT {} FROM {}",
            sel.join(","),
            quote_ident(&format!("cur_{}", fam.moc))
        ));
    }
    if !branches.is_empty() {
        let sql = format!("CREATE VIEW trunk_cur AS {}", branches.join(" UNION ALL "));
        tx.execute_batch(&sql)?;
    }

    // ---- cur_SRVRGT 视图（P7）：SRVRGT 已并入 ASBR，这里保持对外 SQL 口径不变 ----
    if schema.contains_key("ASBR") && schema["ASBR"].iter().any(|c| c.name == "SN") {
        tx.execute_batch(
            "CREATE VIEW cur_SRVRGT AS SELECT \"ne\", \"PUI\" AS \"IMPU\", \"SN\" AS \"SN\" \
             FROM cur_ASBR WHERE \"SN\" IS NOT NULL",
        )?;
        // SQLite 建视图时不校验被引用的表名 —— 这里真查一次，写错了当场炸
        let n: i64 = tx.query_row("SELECT COUNT(*) FROM cur_SRVRGT", [], |r| r.get(0))?;
        debug_assert!(n >= 0);
    }

    // ---- cur_PRALNK 视图（D53 步骤 1）：PRALNK 已并入 PRATG，对外 SQL 口径不变 ----
    //
    // 行数差 2（4,376 vs 4,378）：有 2 行 PRALNK 没有任何 PRATG 行用 LINK 引用它，
    // 它们只存在于内存的 `PratgShim.orphans` 里（dump 会展开，SQL 视图没有它们）。
    if schema.get("PRATG").is_some_and(|c| c.iter().any(|x| x.name == "LNKNAME")) {
        tx.execute_batch(
            "CREATE VIEW cur_PRALNK AS SELECT \"ne\", \"LINK\" AS \"PLN\", \"LNKNAME\", \"SCN\", \
             \"MN\", \"LKS\", \"BINIFID\", \"SIGT\", \"OGCRL\", \"NCF\", \"RCX\", \"CCI\", \"NV\" \
             FROM cur_PRATG WHERE \"LINK\" IS NOT NULL",
        )?;
        let n: i64 = tx.query_row("SELECT COUNT(*) FROM cur_PRALNK", [], |r| r.get(0))?;
        debug_assert!(n >= 0);
    }

    // ---- 中文词典（显示用的旁路维度）：同一个事务里写 ----
    stats.zh_rows = export_zh(lib, &tx)?;

    tx.commit()?;
    Ok(stats)
}

/// 导出**版本链**到 `hist_<MOC>`：一条版本 → 若干 (列, 旧值)，带 `[valid_from, valid_to)`。
///
/// 键值也一起导出（`key` 列，多列用 `|` 连接），这样历史表可以单独查，不必 join `cur_`。
pub fn export_hist(lib: &Library, conn: &mut Connection) -> rusqlite::Result<u64> {
    drop_hist_objects(conn)?;
    let mut schema: BTreeMap<String, (&'static [&'static str], &'static [u8])> = BTreeMap::new();
    for ne in &lib.nes {
        for t in ne.tables() {
            schema.entry(t.moc().to_string()).or_insert_with(|| t.col_meta());
        }
    }
    let mut rows = 0u64;
    let tx = conn.transaction()?;
    for (moc, (names, kinds)) in &schema {
        // (2) 没有任何版本的 MOC 不建表（149 张里约 136 张是空的，空表也要占页）
        let has_versions = lib.nes.iter().any(|ne| {
            ne.table_by_name(moc)
                .map(|t| t.hist_len() > 0)
                .unwrap_or(false)
        });
        if !has_versions {
            continue;
        }
        let tbl = quote_ident(&format!("hist_{moc}"));
        tx.execute_batch(&format!(
            "CREATE TABLE {tbl} (\"ne\" TEXT NOT NULL, \"row\" INTEGER NOT NULL, \"key\" TEXT, \
             \"col\" TEXT NOT NULL, \"old\" TEXT, \"valid_from\" INTEGER NOT NULL, \"valid_to\" INTEGER NOT NULL);"
        ))?;
        let sql = format!("INSERT INTO {tbl} VALUES (?,?,?,?,?,?,?)");
        let mut stmt = tx.prepare(&sql)?;
        for ne in &lib.nes {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let ne_name = ne_name(lib, ne);
            let kcols = t.key_cols();
            for h in t.hist_rows() {
                // 行可能已被删除：键从当前行取（值不因墓碑而消失）
                let mut key = String::new();
                for (j, kc) in kcols.iter().enumerate() {
                    if j > 0 {
                        key.push('|');
                    }
                    let uri_dom = lib.uri_default(
                        lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap_or(usize::MAX),
                        moc,
                    );
                    let cell = cell_of(
                        lib, moc, t, h.row, *kc, kinds[*kc as usize], names[*kc as usize], uri_dom,
                    );
                    match cell {
                        Cell::Null => {}
                        Cell::Int(v) => key.push_str(&v.to_string()),
                        Cell::Text(s) => key.push_str(&s),
                    }
                }
                for (col, old) in &h.cols {
                    let c = *col as usize;
                    let old_cell = old_cell_text(lib, t, moc, *old, *col, kinds[c], names[c]);
                    stmt.execute(rusqlite::params![
                        ne_name,
                        i64::from(h.row),
                        key,
                        names[c],
                        old_cell,
                        h.valid_from,
                        h.valid_to
                    ])?;
                    rows += 1;
                }
            }
        }
    }
    tx.commit()?;
    // hist_<MOC> 的查法固定是 `WHERE ne=?`（回放）或 `WHERE ne=? AND row=?`（下钻），
    // 统一建 (ne, row) 索引。这 13 张表合计只有 2 万量级行，建索引是毫秒级。
    index_hist_moc_tables(conn)?;
    Ok(rows)
}

/// 给所有 `hist_<MOC>` 建 `(ne, row)` 索引（跳过 `hist_row`/`hist_diff`，它们各自建）。
fn index_hist_moc_tables(conn: &Connection) -> rusqlite::Result<()> {
    let mut names: Vec<String> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' \
             AND name LIKE 'hist\\_%' ESCAPE '\\' \
             AND name NOT IN ('hist_row', 'hist_diff')",
        )?;
        let it = st.query_map([], |r| r.get::<_, String>(0))?;
        for x in it {
            names.push(x?);
        }
    }
    for t in names {
        let moc = t.trim_start_matches("hist_");
        conn.execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS {} ON {} (\"ne\", \"row\")",
            quote_ident(&format!("ix_hist_{moc}_ne")),
            quote_ident(&t)
        ))?;
    }
    Ok(())
}

/// 把字符串按 JSON 字符串字面量转义追加（不引入 serde：只有 ASCII 控制符 + `"` + `\\` 三种情况）。
pub(crate) fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `hist_ver` 视图：**每行每次版本一条**，把该版本改动的列聚合成 `列=旧→新` 串。
///
/// `hist_*` / `hist_diff` 是"一行一列一条"的列级明细（`as_of` 回滚要用），
/// 想看"这一版到底改了什么"用这个视图，不必自己 group by：
/// `SELECT * FROM hist_ver WHERE ne='AGCF51' AND moc='CNACLD' LIMIT 5;`
pub fn create_hist_ver_view(conn: &Connection) -> rusqlite::Result<()> {
    // JSON 版：`{"列":"旧→新"}` 太土，直接给 `{"列":{"old":..,"new":..}}`，可用 json_each 拆。
    conn.execute_batch(
        "CREATE VIEW \"hist_ver_json\" AS SELECT \"ne\", \"moc\", \"row\", \"key\", \
         \"valid_from\", \"valid_to\", \
         json_group_object(\"col\", json_object('old', \"old\", 'new', \"new\")) AS \"changes\" \
         FROM \"hist_diff\" GROUP BY \"ne\", \"moc\", \"row\", \"valid_from\", \"valid_to\";",
    )?;
    conn.execute_batch(
        "CREATE VIEW \"hist_ver\" AS SELECT \"ne\", \"moc\", \"row\", \"key\", \
         \"valid_from\", \"valid_to\", COUNT(*) AS \"n_cols\", \
         group_concat(\"col\" || '=' || COALESCE(\"old\", '(空)') || '→' || \
                      COALESCE(\"new\", '(空)'), char(31)) AS \"changes\" \
         FROM \"hist_diff\" GROUP BY \"ne\", \"moc\", \"row\", \"valid_from\", \"valid_to\";",
    )?;
    Ok(())
}

/// **行级版本**（方案 a，对齐 ims_core 的 `ent_ver.exists_`）：每行一条生命周期。
///
/// 为什么需要它：删行如果按"每列一条旧值"写进 `hist_<MOC>`，269,053 个删行就是 511 万条记录
/// （占整个 hist 的 99.6%），而 `as_of` 根本用不到 —— 它从当前行结构出发回滚，删行不清结构、
/// 同键再现又分配新行号，所以那 511 万条全是冗余。
///
/// 表结构：`(ne, moc, row, key, created_from, dead_at, image)`
/// - `dead_at` 为 `NULL` = 还活着；
/// - `image` 只有死行才有：`列名=渲染值` 用 `\x1f` 连接（`NULL` 列省略），
///   也就是"它死的时候长什么样"—— 这样压缩是**无损**的，不是把内容丢掉。
pub fn export_hist_row(lib: &Library, conn: &mut Connection) -> rusqlite::Result<u64> {
    let mut schema: BTreeMap<String, (&'static [&'static str], &'static [u8])> = BTreeMap::new();
    for ne in &lib.nes {
        for t in ne.tables() {
            schema.entry(t.moc().to_string()).or_insert_with(|| t.col_meta());
        }
    }
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE \"hist_row\" (\"ne\" TEXT NOT NULL, \"moc\" TEXT NOT NULL, \
         \"row\" INTEGER NOT NULL, \"key\" TEXT, \"created_from\" INTEGER NOT NULL, \
         \"dead_at\" INTEGER, \"image\" TEXT);",
    )?;
    let mut stmt = tx.prepare("INSERT INTO \"hist_row\" VALUES (?,?,?,?,?,?,?)")?;
    let mut rows = 0u64;
    for (moc, (names, kinds)) in &schema {
        for ne in &lib.nes {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let ne_name = ne_name(lib, ne);
            let ne_idx = lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap_or(usize::MAX);
            let uri_dom = lib.uri_default(ne_idx, moc);
            let kcols = t.key_cols();
            let first = t.first_batch();
            for (row, created_from, dead_at) in t.lifecycles() {
                // (1) 首批创建且仍活着 = 平凡生命周期，不写（它就在 cur_* 里）
                if dead_at.is_none() && created_from == first {
                    continue;
                }
                let mut key = String::new();
                for (j, kc) in kcols.iter().enumerate() {
                    if j > 0 {
                        key.push('|');
                    }
                    match cell_of(lib, moc, t, row, *kc, kinds[*kc as usize], names[*kc as usize], uri_dom) {
                        Cell::Null => {}
                        Cell::Int(v) => key.push_str(&v.to_string()),
                        Cell::Text(s) => key.push_str(&s),
                    }
                }
                // 死行镜像：只有死了才写（活着的内容 cur_* 里就有）。格式 = 标准 JSON 对象，
                // 如 `{"PUI":"sip:+86...","DID":"V5ST"}`；`NULL` 列省略。
                let image = dead_at.map(|_| {
                    let mut img = String::from("{");
                    let mut first = true;
                    for c in 0..names.len() {
                        // NULL 列省略 —— 必须在写分隔符/键名**之前**判断
                        let cell = cell_of(lib, moc, t, row, c as u16, kinds[c], names[c], uri_dom);
                        if matches!(cell, Cell::Null) {
                            continue;
                        }
                        if !first {
                            img.push(',');
                        }
                        first = false;
                        crate::push_json_str(&mut img, names[c]);
                        img.push(':');
                        match cell {
                            Cell::Null => {}
                            Cell::Int(v) => img.push_str(&v.to_string()),
                            Cell::Text(s) => crate::push_json_str(&mut img, &s),
                        }
                    }
                    img.push('}');
                    img
                });
                stmt.execute(rusqlite::params![
                    ne_name, moc, i64::from(row), key, created_from, dead_at, image
                ])?;
                rows += 1;
            }
        }
    }
    drop(stmt);
    tx.commit()?;
    // `hist_row` 的查法：按 (ne, moc) 找某网元某表的所有生命周期
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS \"ix_hist_row_ne\" ON \"hist_row\" (\"ne\", \"moc\")",
    )?;
    Ok(rows)
}

/// **逐行 diff 明细**（P5c）：把 `hist_<MOC>` 的 (版本, 列) 旧值串成 `old → new`。
///
/// 口径（与 `hist_<MOC>` 同源，不重算版本）：
/// - 一条记录 = (网元, MOC, 行, 列, 版本)：`old` = 该版本开始时被替换掉的值；
/// - `new` = 同 (行, 列) 下一条版本的 `old`；若是该列最后一条版本，取**当前值**
///   （用 `cur_*` 同一套渲染口径，所以和 `cur_` 对得上）；
/// - `valid_from`/`valid_to` 照抄 `hist_<MOC>`（同一条版本区间）。
///
/// 为什么有用：`hist_<MOC>` 是 (版本, 列) 粒度，要回答"这次改了什么、改成什么"
/// 得自己 join/pivot；这张表直接给 old→new，按 (ne, moc, row, valid_from) 分组
/// 就是"该行这一版改了哪几列"。
/// hist_diff 的中间形态：(列号, valid_from, valid_to, 旧值)。
type DiffRec = (u16, i64, i64, Option<String>);

pub fn export_hist_diff(lib: &Library, conn: &mut Connection) -> rusqlite::Result<u64> {
    let mut schema: BTreeMap<String, (&'static [&'static str], &'static [u8])> = BTreeMap::new();
    for ne in &lib.nes {
        for t in ne.tables() {
            schema.entry(t.moc().to_string()).or_insert_with(|| t.col_meta());
        }
    }
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE \"hist_diff\" (\"ne\" TEXT NOT NULL, \"moc\" TEXT NOT NULL, \
         \"row\" INTEGER NOT NULL, \"key\" TEXT, \"col\" TEXT NOT NULL, \"old\" TEXT, \
         \"new\" TEXT, \"valid_from\" INTEGER NOT NULL, \"valid_to\" INTEGER NOT NULL);",
    )?;
    let mut stmt = tx.prepare("INSERT INTO \"hist_diff\" VALUES (?,?,?,?,?,?,?,?,?)")?;
    let mut rows = 0u64;
    for (moc, (names, kinds)) in &schema {
        for ne in &lib.nes {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let ne_name = ne_name(lib, ne);
            let ne_idx = lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap_or(usize::MAX);
            let uri_dom = lib.uri_default(ne_idx, moc);
            let kcols = t.key_cols();
            // 本表本网元的版本记录，按行分组（每张表的历史量是有界的，逐表处理不会顶上去）
            let mut per_row: std::collections::HashMap<u32, Vec<DiffRec>> =
                std::collections::HashMap::new();
            for h in t.hist_rows() {
                let e = per_row.entry(h.row).or_default();
                for (col, old) in &h.cols {
                    let c = *col as usize;
                    let old_cell = old_cell_text(lib, t, moc, *old, *col, kinds[c], names[c]);
                    e.push((*col, h.valid_from, h.valid_to, old_cell));
                }
            }
            for (row, mut recs) in per_row {
                recs.sort_by_key(|(c, vf, _, _)| (*c, *vf));
                // 键（行可能已删除，键从当前行取，与 hist_<MOC> 一致）
                let mut key = String::new();
                for (j, kc) in kcols.iter().enumerate() {
                    if j > 0 {
                        key.push('|');
                    }
                    match cell_of(lib, moc, t, row, *kc, kinds[*kc as usize], names[*kc as usize], uri_dom) {
                        Cell::Null => {}
                        Cell::Int(v) => key.push_str(&v.to_string()),
                        Cell::Text(s) => key.push_str(&s),
                    }
                }
                let mut i = 0;
                while i < recs.len() {
                    let col = recs[i].0;
                    let mut j = i;
                    while j < recs.len() && recs[j].0 == col {
                        j += 1;
                    }
                    for k in i..j {
                        let (_, vf, vt, old) = &recs[k];
                        let new = if k + 1 < j {
                            recs[k + 1].3.clone()
                        } else {
                            match cell_of(lib, moc, t, row, col, kinds[col as usize], names[col as usize], uri_dom) {
                                Cell::Null => None,
                                Cell::Int(v) => Some(v.to_string()),
                                Cell::Text(x) => Some(x),
                            }
                        };
                        stmt.execute(rusqlite::params![
                            ne_name,
                            moc,
                            i64::from(row),
                            key,
                            names[col as usize],
                            old,
                            new,
                            vf,
                            vt
                        ])?;
                        rows += 1;
                    }
                    i = j;
                }
            }
        }
    }
    drop(stmt);
    tx.commit()?;
    // `hist_diff` 的查法：按 (ne, moc) 取一张表，再按 (row, col) 串 old→new 链
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS \"ix_hist_diff_ne\" ON \"hist_diff\" \
         (\"ne\", \"moc\", \"row\", \"col\")",
    )?;
    Ok(rows)
}

/// `ColVal` → 文本（历史表用统一文本表示，`NULL` 保持 `None`）。
fn old_cell_text(
    lib: &Library,
    t: &dyn TableApi,
    moc: &str,
    v: ims_mem2::ColVal,
    col: u16,
    kind: u8,
    name: &str,
) -> Option<String> {
    if v.is_default() {
        return None;
    }
    match kind {
        // A3：字冠伴生列 —— 池下标 → `K'` + 数字
        b'N' => v
            .as_group()
            .and_then(|idx| t.pfx_at(idx))
            .map(ims_mem2::frombytes::render_pfx),
        // A2：宽值列 —— 池下标 → 渲染；原文兜底 → 原文
        b'i' | b'd' | b'x' | b'p' | b'c' => match v.as_group() {
            Some(idx) => t.wide_at(idx).and_then(|w| t.render_wide(col, w)),
            None => v.as_sym().map(|s| lib.interner.decode(s)),
        },
        b't' => v.as_sym().map(|s| lib.interner.decode(s)),
        b'b' => v.as_u32().map(|x| if x != 0 { "1".into() } else { "0".into() }),
        b'e' => {
            let pool = lib.pools.find(&format!("{moc}.{name}")).unwrap_or(usize::MAX);
            v.as_u32()
                .and_then(|x| ims_mem2::Label::from_u8_0(x as u8))
                .and_then(|l| lib.pools.label_sym(pool, l))
                .map(|s| lib.interner.decode(s))
        }
        _ => v.as_u32().map(|x| x.to_string()),
    }
}

/// DB 侧的"表 → 规范行串哈希"，与 [`mem_hashes`] 同口径。
pub fn db_hashes(path: &str, schema: &BTreeMap<String, Vec<String>>) -> rusqlite::Result<BTreeMap<String, (u64, u64)>> {
    let conn = Connection::open(path)?;
    let mut out = BTreeMap::new();
    for (moc, cols) in schema {
        let sql = format!(
            "SELECT \"ne\",{} FROM {}",
            cols.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(","),
            quote_ident(&format!("cur_{moc}"))
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows: Vec<String> = Vec::new();
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            let ne: String = r.get(0)?;
            let mut s = String::with_capacity(64);
            s.push_str(&ne);
            s.push('|');
            for i in 0..cols.len() {
                let v: rusqlite::types::Value = r.get(i + 1)?;
                let cell = match v {
                    rusqlite::types::Value::Null => Cell::Null,
                    rusqlite::types::Value::Integer(v) => Cell::Int(v),
                    rusqlite::types::Value::Real(f) => Cell::Int(f as i64),
                    rusqlite::types::Value::Text(t) => Cell::Text(t),
                    rusqlite::types::Value::Blob(_) => Cell::Null,
                };
                cell.write(&mut s);
            }
            rows.push(s);
        }
        rows.sort();
        let joined = rows.join("\n");
        out.insert(moc.clone(), (rows.len() as u64, fnv1a64(joined.as_bytes())));
    }
    Ok(out)
}

/// 从内存里取 `(MOC → 列名)`，供 [`db_hashes`] 使用。
pub fn schema_of(lib: &Library) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    for ne in &lib.nes {
        for t in ne.tables() {
            m.entry(t.moc().to_string())
                .or_insert_with(|| expand_cols(t).iter().map(|c| c.name.clone()).collect());
        }
    }
    m
}

/// 导出用的 `Sym` → 原文（方便外部调用者）。
pub fn decode(lib: &Library, s: Sym) -> String {
    lib.interner.decode(s)
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    /// 复用同一个库文件时，导出必须幂等：旧 cur_* / trunk_cur / hist_* 先清掉，不撞 already exists。
    #[test]
    fn drop_projection_objects_is_idempotent() {
        // 用 workspace 的 target 目录（%TEMP% 在沙箱里禁止写）
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test_tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("ims_drop_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE cur_FOO (x INTEGER); \
                 CREATE INDEX ix_cur_FOO_ne ON cur_FOO(x); \
                 CREATE VIEW trunk_cur AS SELECT 1 AS x; \
                 CREATE TABLE hist_BAR (y INTEGER); \
                 CREATE INDEX ix_hist_BAR_ne ON hist_BAR(y); \
                 CREATE TABLE hist_row (z INTEGER);",
            )
            .unwrap();
            drop_cur_objects(&conn).unwrap();
            drop_hist_objects(&conn).unwrap();
            let left: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name IN \
                     ('cur_FOO','trunk_cur','hist_BAR','hist_row', \
                      'ix_cur_FOO_ne','ix_hist_BAR_ne')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(left, 0, "旧对象（表/视图/索引）应被清干净");
            // 再跑一遍：幂等（不报错）
            drop_cur_objects(&conn).unwrap();
            drop_hist_objects(&conn).unwrap();
        }
        let _ = std::fs::remove_file(&path);
    }
}
