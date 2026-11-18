//! 极简 HTTP GET 服务器（纯 std）。
//!
//! 职责：① 内嵌静态前端（index.html / style.css / app.js）；② 把 `/api/*`
//! 的查询参数包装成 `ims_svc` 的 JSON 请求转发（走 `SvcClient`），原样返回其 JSON。
//! 每连接一线程；连接复用 + Mutex 串行保证转发不交错。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use crate::svc::SvcClient;

const INDEX_HTML: &str = include_str!("static/index.html");
const STYLE_CSS: &str = include_str!("static/style.css");
const APP_JS: &str = include_str!("static/app.js");

pub fn serve_http(svc: Arc<SvcClient>, addr: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    eprintln!("ims_web 监听 http://{addr}（转发到 ims_svc，GET 服务；Ctrl+C 退出）");
    for conn in listener.incoming() {
        let Ok(s) = conn else { continue };
        let svc = svc.clone();
        std::thread::spawn(move || {
            let _ = handle(s, svc);
        });
    }
    Ok(())
}

fn handle(s: TcpStream, svc: Arc<SvcClient>) -> std::io::Result<()> {
    s.set_read_timeout(Some(std::time::Duration::from_secs(30))).ok();
    let mut rd = BufReader::new(s.try_clone()?);
    // 请求行：GET /path HTTP/1.x
    let mut req_line = String::new();
    if rd.read_line(&mut req_line)? == 0 {
        return Ok(());
    }
    // 头部：读到空行即止（GET 无 body）
    loop {
        let mut h = String::new();
        if rd.read_line(&mut h)? == 0 {
            break;
        }
        if h.trim().is_empty() {
            break;
        }
    }

    let (path, query) = parse_request_line(&req_line);
    let resp = route(&path, &query, &svc);
    let mut s = s;
    s.write_all(&resp)?;
    s.flush()
}

/// 解析 "GET /path?query HTTP/1.x"
fn parse_request_line(line: &str) -> (String, String) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let target = parts.get(1).copied().unwrap_or("/");
    match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    }
}

fn route(path: &str, query: &str, svc: &Arc<SvcClient>) -> Vec<u8> {
    match path {
        "/" | "/index.html" => resp(200, "text/html; charset=utf-8", INDEX_HTML),
        "/style.css" => resp(200, "text/css; charset=utf-8", STYLE_CSS),
        "/app.js" => resp(200, "application/javascript; charset=utf-8", APP_JS),
        "/api/hello" => resp(200, "application/json", &svc.call(r#"{"q":"hello","v":1}"#)),
        "/api/catalog" => {
            let q = catalog_q(query);
            resp(200, "application/json", &svc.call(&q))
        }
        "/api/object" => {
            let q = object_req(query);
            resp(200, "application/json", &svc.call(&q))
        }
        "/api/link" => {
            let q = link_req(query);
            resp(200, "application/json", &svc.call(&q))
        }
        "/favicon.ico" => resp(204, "text/plain", ""),
        _ => resp(
            404,
            "text/plain; charset=utf-8",
            "not found\n服务: /  /style.css  /app.js  /api/hello|catalog|object|link",
        ),
    }
}

fn resp(status: u16, ctype: &str, body: &str) -> Vec<u8> {
    let reason = if status == 200 {
        "OK"
    } else if status == 404 {
        "Not Found"
    } else {
        "No Content"
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// JSON 字符串转义（值里出现的引号/反斜杠）
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

/// URL 解码：%XX 与 '+' → ' '（仅解码值，足够参数用）
fn percent_decode(s: &str) -> String {
    let b: Vec<u8> = s.as_bytes().to_vec();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = hex(b[i + 1]);
                let l = hex(b[i + 2]);
                if let (Some(h), Some(l)) = (h, l) {
                    out.push(h * 16 + l);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// 把 query 字符串解析成 参数表
fn parse_query(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    query
        .split('&')
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.to_string(), percent_decode(v))
        })
        .collect()
}

fn get<'a>(params: &'a [(String, String)], k: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(a, _)| a == k)
        .map(|(_, v)| v.as_str())
}

fn catalog_q(query: &str) -> String {
    let p = parse_query(query);
    let pool = get(&p, "pool").unwrap_or("prefix");
    let kind = get(&p, "kind").unwrap_or("prefix");
    let arg = get(&p, "arg").unwrap_or("");
    let limit: u64 = get(&p, "limit").and_then(|s| s.parse().ok()).unwrap_or(50).min(500);
    let mut body = format!(
        r#""q":"catalog","pool":"{}","kind":"{}","arg":"{}","limit":{}"#,
        esc(pool),
        esc(kind),
        esc(arg),
        limit
    );
    // after 两级游标（可选）：[match, key]
    if let Some(a) = get(&p, "after") {
        if !a.is_empty() {
            let mut seg = a.splitn(2, '|');
            let m = seg.next().unwrap_or("");
            let k = seg.next().unwrap_or("");
            body.push_str(&format!(r#","after":["{}","{}"]"#, esc(m), esc(k)));
        }
    }
    format!("{{{body}}}")
}

fn object_req(query: &str) -> String {
    let p = parse_query(query);
    let io = get(&p, "io").unwrap_or("");
    let ne: i64 = get(&p, "ne").and_then(|s| s.parse().ok()).unwrap_or(1);
    let key = get(&p, "key").unwrap_or("");
    format!(
        r#"{{"q":"object","io":"{}","ne":{},"key":"{}"}}"#,
        esc(io),
        ne,
        esc(key)
    )
}

fn link_req(query: &str) -> String {
    let p = parse_query(query);
    let io = get(&p, "io").unwrap_or("");
    let field = get(&p, "field").unwrap_or("");
    let ne: i64 = get(&p, "ne").and_then(|s| s.parse().ok()).unwrap_or(1);
    let key = get(&p, "key").unwrap_or("");
    format!(
        r#"{{"q":"link","io":"{}","field":"{}","ne":{},"key":"{}"}}"#,
        esc(io),
        esc(field),
        ne,
        esc(key)
    )
}
