//! 线格式·客户端侧（D64/D66）：解析服务端返回的一行 JSON。
//! 应答的构造在服务端（crates/svc/src/proto.rs 的 `dispatch`）。

use serde_json::Value;

pub struct Row {
    /// 展示键（字冠池 = 原字冠；用户池 = nsn；trunk = tg）
    pub key: String,
    /// 剥冠后的核心（协议语义保留；当前 UI 用 `mkey` 展示命中，core 供后续/详情用）
    #[allow(dead_code)]
    pub core: Option<String>,
    pub mkey: Option<String>,
    pub roles: String,
    pub nes: Vec<i64>,
    pub hits: u32,
    pub lands: Option<bool>,
}

pub struct CatPage {
    pub total: usize,
    /// 实际查询键（服务端剥冠后）
    pub query: String,
    pub rows: Vec<Row>,
}

/// 一个开关位的明细：(位名, 是否置位, 中文说明)。
pub type BitItem = (String, bool, Option<String>);

/// 一个开关型字段的明细：(字段名, 各位)。
pub type BitsDetail = (String, Vec<BitItem>);

#[derive(Clone)]
pub struct ObjView {
    pub io: String,
    pub ne: i64,
    pub key: String,
    /// (名称, 值, 中文名) —— 中文名是新增的第三元（老服务端没有就是 None）
    pub fields: Vec<(String, Option<String>, Option<String>)>,
    pub links: Vec<(String, String, usize)>,
    /// 开关型参数的逐位明细：(字段名, [(位名, 是否置位, 中文)])（D67 的 S 层）
    pub bits: Vec<BitsDetail>,
}

pub fn parse_catalog(line: &str) -> Result<CatPage, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return Err(v
            .pointer("/error/message")
            .and_then(|x| x.as_str())
            .unwrap_or("catalog 失败")
            .to_string());
    }
    let mut p = CatPage {
        total: v.get("total").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        query: v
            .get("query")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        rows: Vec::new(),
    };
    if let Some(items) = v.get("items").and_then(|x| x.as_array()) {
        for it in items {
            p.rows.push(Row {
                key: it.get("key").and_then(|x| x.as_str()).unwrap_or("").into(),
                core: it.get("core").and_then(|x| x.as_str()).map(String::from),
                mkey: it.get("match").and_then(|x| x.as_str()).map(String::from),
                // The response no longer carries roles_str (dropped in D79);
                // rebuild it from the roles array.
                roles: it
                    .get("roles")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .unwrap_or_default(),
                nes: it
                    .get("nes")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().filter_map(|n| n.as_i64()).collect())
                    .unwrap_or_default(),
                hits: it.get("hits").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                lands: it.get("lands").and_then(|x| x.as_bool()),
            });
        }
    }
    Ok(p)
}

pub fn parse_object(line: &str) -> Result<ObjView, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return Err(v
            .pointer("/error/message")
            .and_then(|x| x.as_str())
            .unwrap_or("object 失败")
            .to_string());
    }
    let mut o = ObjView {
        io: v.get("io").and_then(|x| x.as_str()).unwrap_or("").into(),
        ne: v.get("ne").and_then(|x| x.as_i64()).unwrap_or(0),
        key: v.get("key").and_then(|x| x.as_str()).unwrap_or("").into(),
        fields: Vec::new(),
        links: Vec::new(),
        bits: Vec::new(),
    };
    if let Some(fs) = v.get("fields").and_then(|x| x.as_array()) {
        for f in fs {
            if let Some(pair) = f.as_array() {
                let k = pair.first().and_then(|x| x.as_str()).unwrap_or("").to_string();
                let val = pair.get(1).and_then(|x| x.as_str()).map(|s| s.to_string());
                let cn = pair.get(2).and_then(|x| x.as_str()).map(|s| s.to_string());
                o.fields.push((k, val, cn));
            }
        }
    }
    if let Some(ls) = v.get("links").and_then(|x| x.as_array()) {
        for l in ls {
            if let Some(t) = l.as_array() {
                o.links.push((
                    t.first().and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    t.get(1).and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    t.get(2).and_then(|x| x.as_u64()).unwrap_or(0) as usize,
                ));
            }
        }
    }
    // 开关型参数的逐位明细（D67 的 S 层）：`[{"col":字段名,"items":[[位名,0|1,中文]]}]`
    if let Some(bs) = v.get("bits").and_then(|x| x.as_array()) {
        for b in bs {
            let col = b.get("col").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let mut items = Vec::new();
            for it in b.get("items").and_then(|x| x.as_array()).map(|x| x.as_slice()).unwrap_or(&[]) {
                let Some(t) = it.as_array() else { continue };
                items.push((
                    t.first().and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    t.get(1).and_then(|x| x.as_i64()).unwrap_or(0) != 0,
                    t.get(2).and_then(|x| x.as_str()).map(String::from),
                ));
            }
            o.bits.push((col, items));
        }
    }
    Ok(o)
}

pub fn parse_links(line: &str) -> Result<Vec<(String, i64, String)>, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return Err(v
            .pointer("/error/message")
            .and_then(|x| x.as_str())
            .unwrap_or("link 失败")
            .to_string());
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
    Ok(out)
}



