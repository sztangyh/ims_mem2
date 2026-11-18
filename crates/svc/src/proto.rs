//! 线格式·服务端侧（D64/D66）：一行一个 JSON 对象。
//!
//! 本模块只做**应答**（`dispatch`）；响应的解析在客户端（crates/tui/src/proto.rs）。
//! 自检用的 `parse_*_local` 只取自检需要的字段，不复刻完整客户端解析。

use serde_json::{json, Value};

use crate::engine::{Engine, Obj};

/// 处理一行请求，返回一行响应。
pub fn dispatch(eng: &Engine, line: &str) -> String {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return err("bad_request", &format!("JSON 解析失败: {e}"), Value::Null),
    };
    let q = v.get("q").and_then(|x| x.as_str()).unwrap_or("");
    match q {
        "hello" => json!({
            "v": 1, "ok": true, "server": "ims-svc/0.2",
            "ios": 20, "pools": ["prefix", "subscriber", "trunk"],
            "limits": {"page_default": 50, "page_max": 500},
            "caps": ["catalog", "object", "link", "meta", "prefix_profile", "crown_strip"]
        })
        .to_string(),
        "catalog" => {
            let pool = v.get("pool").and_then(|x| x.as_str()).unwrap_or("trunk");
            let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("prefix");
            let arg = v.get("arg").and_then(|x| x.as_str()).unwrap_or("");
            let limit = v
                .get("limit")
                .and_then(|x| x.as_u64())
                .unwrap_or(50)
                .min(500) as usize;
            let after: Option<(String, String)> = v
                .get("after")
                .and_then(|a| a.as_array())
                .and_then(|a| {
                    Some((
                        a.first()?.as_str()?.to_string(),
                        a.get(1).and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    ))
                });
            let page = eng.catalog(
                pool,
                kind,
                arg,
                limit,
                after.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
            );
            // 条目只带**不可推导**的字段：
            //   `id`（= pool|key）、`roles_str`（= roles.join("/")）都是冗余，已删。
            //   客户端要用就自己拼（web/tui 都不需要）。
            let arr: Vec<Value> = page
                .items
                .iter()
                .map(|e| {
                    json!({
                        "key": e.key,
                        "core": e.core,
                        "match": e.mkey,
                        "roles": e.roles,
                        "nes": e.nes,
                        "hits": e.hits,
                        "lands": e.lands,
                    })
                })
                .collect();
            let next = page
                .next
                .as_ref()
                .map(|(m, k)| json!([m, k]))
                .unwrap_or(Value::Null);
            // `raw` 与 `arg` 恒等，删掉；`query` 是剥冠后的实际查询键，保留
            json!({"v":1,"ok":true,"pool":pool,"kind":kind,"arg":arg,
                   "query":page.query,
                   "total":page.total,"items":arr,"next_cursor":next})
            .to_string()
        }
        "object" => {
            let io = v.get("io").and_then(|x| x.as_str()).unwrap_or("");
            let ne = v.get("ne").and_then(|x| x.as_i64()).unwrap_or(1);
            let key = v.get("key").and_then(|x| x.as_str()).unwrap_or("");
            match eng.object(io, ne, key) {
                Some(o) => obj_json_x(&o, eng.zh(), if Engine::is_mgw_io(io) { eng.mgw_zh() } else { None }),
                None => {
                    // 同一个键可能挂在别的网元：依次试
                    let mut hit = None;
                    for n in 1..=6 {
                        if let Some(o) = eng.object(io, n, key) {
                            hit = Some(o);
                            break;
                        }
                    }
                    match hit {
                        Some(o) => obj_json_x(&o, eng.zh(), if Engine::is_mgw_io(io) { eng.mgw_zh() } else { None }),
                        None => err(
                            "not_found",
                            "object not found",
                            json!({"io": io, "ne": ne, "key": key}),
                        ),
                    }
                }
            }
        }
        "link" => {
            let io = v.get("io").and_then(|x| x.as_str()).unwrap_or("");
            let field = v.get("field").and_then(|x| x.as_str()).unwrap_or("");
            let ne = v.get("ne").and_then(|x| x.as_i64()).unwrap_or(1);
            let key = v.get("key").and_then(|x| x.as_str()).unwrap_or("");
            let targets = eng.expand(io, field, ne, key);
            let arr: Vec<Value> = targets
                .iter()
                .map(|(t, n, k)| json!({"io": t, "ne": n, "key": k}))
                .collect();
            json!({"v":1,"ok":true,"from":io,"field":field,"ne":ne,"key":key,"targets":arr})
                .to_string()
        }
        "meta.pools" => json!({
            "v":1,"ok":true,"pools":["prefix","subscriber","trunk"],
            "note":"分页只用于目录（D61）"
        })
        .to_string(),
        "quit" => json!({"v":1,"ok":true,"bye":true}).to_string(),
        other => err(
            "bad_request",
            &format!("未知方法: {other:?}"),
            json!({"q": other}),
        ),
    }
}

/// 字段名 → MOC 参数名（**只有名字对不上的才需要**一行）。
///
/// `io_spec` 的字段名是给外部看的短名（`mgw_mid`），而词典按 MOC 参数名（`MGW`）索引；
/// 其余字段（`tg`/`binifid`/`lks`/`csc`/`eid`/`tgn`）大小写归一后就能直接命中
/// （`binifid`/`lks` 在 `zh_param` 里没有，但 `zh_param_any` 有）。实测 `PRATG.MGW` =「设备标识」。
fn zh_alias(field: &str) -> &str {
    match field {
        "mgw_mid" => "MGW",
        _ => field,
    }
}

/// 序列化一个对象；`fields` 每项是 `[名称, 值, 中文名]`。
///
/// 第三个元素是**新增的**（老客户端只读 `[0]`/`[1]`，不受影响）。
/// 中文名来自 `zh.tsv`：先按对象的主来源 MOC 查，再按参数名跨 MOC 兜底；
/// 都查不到就是 `null`（不猜）。
/// `no_zh = true` 时不贴中文名。
///
/// MGW 对象的字段名（`mid` / `bt` / `bn` …）与 AGCF 参数**同名但不同义**，而 `zh.tsv` 只有 AGCF 的 MOC；
/// `ZhDict::param_cn` 找不到本 MOC 时会按参数名兜底到"任意 MOC"，于是 `mid` 会被贴上 AGCF 的「ACU模块号」。
/// 宁可空着 —— 错的中文名比没有中文名更糟。
fn obj_json_x(
    o: &Obj,
    zh: &ims_mem2::ZhDict,
    mgw_zh: Option<&ims_mem2::mgw::MgwZh>,
) -> String {
    let fields: Vec<Value> = o
        .fields
        .iter()
        .map(|(k, v)| {
            // MGW 对象走 **MGW 自己的** 词典（两边参数同名不同义，绝不能落到 AGCF 的 `param_cn`）
            let cn: Option<String> = match mgw_zh {
                Some(z) => z.param_cn(o.moc, k).map(|s| s.to_string()),
                None => zh
                    .param_cn(o.moc, k)
                    .or_else(|| zh.param_cn(o.moc, zh_alias(k)))
                    .map(|s| s.to_string()),
            };
            let cn = cn.as_deref();
            json!([k, v.as_deref(), cn])
        })
        .collect();
    let links: Vec<Value> = o
        .links
        .iter()
        .map(|(f, c, n)| json!([f, c, n]))
        .collect();
    // 开关型参数的逐位明细：`[{"col":字段名,"items":[[位名, 0|1, 中文说明]]}]`
    // 只在有 Bits 列时出现（其余对象是空数组，客户端可忽略）
    let bits: Vec<Value> = o
        .bits
        .iter()
        .map(|(col, items)| {
            let arr: Vec<Value> = items
                .iter()
                .map(|(n, on, cn)| json!([n, if *on { 1 } else { 0 }, cn]))
                .collect();
            json!({"col": col, "items": arr})
        })
        .collect();
    json!({"v":1,"ok":true,"io":o.io,"ne":o.ne,"key":o.key,
           "fields":fields,"links":links,"bits":bits,"detail":o.detail})
    .to_string()
}

fn err(code: &str, msg: &str, detail: Value) -> String {
    json!({"v":1,"ok":false,"error":{"code":code,"message":msg,"detail":detail}}).to_string()
}

// ---------------- 自检用的极简解析（只取自检打印需要的字段） ----------------

pub struct CatLite {
    pub total: usize,
    pub query: String,
    pub keys: Vec<String>,
    pub roles: Vec<String>,
    pub lands: Vec<bool>,
}

pub fn parse_catalog_local(line: &str) -> Result<CatLite, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return Err(v
            .pointer("/error/message")
            .and_then(|x| x.as_str())
            .unwrap_or("catalog 失败")
            .to_string());
    }
    let total = v.get("total").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
    let query = v
        .get("query")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let mut keys = Vec::new();
    let mut roles = Vec::new();
    let mut lands = Vec::new();
    if let Some(items) = v.get("items").and_then(|x| x.as_array()) {
        for it in items {
            keys.push(it.get("key").and_then(|x| x.as_str()).unwrap_or("").to_string());
            // 响应里已不再带 `roles_str`（冗余），这里从 `roles` 数组拼回来，
            // 保证自检/对拍的输出与之前逐字相同
            roles.push(
                it.get("roles")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .unwrap_or_default(),
            );
            lands.push(it.get("lands").and_then(|x| x.as_bool()).unwrap_or(false));
        }
    }
    Ok(CatLite { total, query, keys, roles, lands })
}

pub struct ObjLite {
    pub ne: i64,
    /// `(名称, 值, 中文名)` —— 中文名是 D67 加的第三元，`parse_object_local` 解析它，
    /// 这样 `ims_svc --selftest` 与 `ims_tui --tcp --selftest` 打出来的行才逐字相同
    pub fields: Vec<(String, Option<String>, Option<String>)>,
    pub links: Vec<(String, String, usize)>,
}

pub fn parse_object_local(line: &str) -> Option<ObjLite> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return None;
    }
    let mut fields = Vec::new();
    if let Some(fs) = v.get("fields").and_then(|x| x.as_array()) {
        for f in fs {
            if let Some(pair) = f.as_array() {
                fields.push((
                    pair.first().and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    pair.get(1).and_then(|x| x.as_str()).map(String::from),
                    pair.get(2).and_then(|x| x.as_str()).map(String::from),
                ));
            }
        }
    }
    let mut links = Vec::new();
    if let Some(ls) = v.get("links").and_then(|x| x.as_array()) {
        for l in ls {
            if let Some(t) = l.as_array() {
                links.push((
                    t.first().and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    t.get(1).and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    t.get(2).and_then(|x| x.as_u64()).unwrap_or(0) as usize,
                ));
            }
        }
    }
    Some(ObjLite {
        ne: v.get("ne").and_then(|x| x.as_i64()).unwrap_or(0),
        fields,
        links,
    })
}

pub fn parse_links_local(line: &str) -> Option<Vec<(String, i64, String)>> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return None;
    }
    let mut out = Vec::new();
    if let Some(ts) = v.get("targets").and_then(|x| x.as_array()) {
        for t in ts {
            out.push((
                t.get("io").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                t.get("ne").and_then(|x| x.as_i64()).unwrap_or(0),
                t.get("key").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            ));
        }
    }
    Some(out)
}



