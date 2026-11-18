//! GBK 字节级后备文件扫描器：把一份后备 `.txt` 变成事件流。
//!
//! 移植自 `ims_mem/src/scan.rs`（已被 6 网元 × 47.5 万行实测过），只把数值解析换成
//! 本 crate 的 [`crate::frombytes`]。口径（AGCF.md / hw_mml.md）：
//! - 文件头 `/* TIME = y-m-d h:m:s */` → 批次号（距 2000-01-01 的分钟数）；
//! - `USE ME:MEID=n;` 切换网元段：MEID=0 是 CGP（站点管理段），其余是业务网元段；
//! - MOC 子块按**命令对象**分组（块头注释名会撒谎，不依赖注释名）；
//! - 参数按名匹配；引号内逗号不分隔；键/值 trim，值保留原文；
//! - 护栏：非空、非注释、非合法 MML 的行 → `Event::Odd` 主动上报（不静默丢）。

use crate::frombytes::parse_u32;

/// MML 操作符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Mod,
    Rmv,
    Set,
    /// 其它（ACT/DEA/BLK/UBL/RST/STP/SWP/USE …），按名识别、按需扩展。
    Other,
}

impl Op {
    pub fn parse(s: &[u8]) -> Op {
        if s.eq_ignore_ascii_case(b"ADD") {
            Op::Add
        } else if s.eq_ignore_ascii_case(b"MOD") {
            Op::Mod
        } else if s.eq_ignore_ascii_case(b"RMV") {
            Op::Rmv
        } else if s.eq_ignore_ascii_case(b"SET") {
            Op::Set
        } else {
            Op::Other
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Op::Add => "ADD",
            Op::Mod => "MOD",
            Op::Rmv => "RMV",
            Op::Set => "SET",
            Op::Other => "OTHER",
        }
    }
}

/// 扫描事件。
#[derive(Debug)]
pub enum Event<'a> {
    /// 文件头 TIME（首个 TIME 生效）。
    Time {
        /// 形如 `2026-09-21 03:30:30`
        text: &'a [u8],
        /// 距 2000-01-01 的分钟数（批次号）
        batch_no: i64,
    },
    /// 网元段头元信息（`/* ME Type=CGP, Version=... */`）
    MeHeader {
        me_type: &'a [u8],
        version: &'a [u8],
    },
    /// `USE ME:MEID=n`（段开始；CGP 段 meid=0）
    Segment { meid: u32 },
    /// 一条 MML 命令行（body 为 `:` 与 `;` 之间、两端已 trim）
    Mml {
        op: Op,
        object: &'a [u8],
        body: &'a [u8],
        line_no: u32,
    },
    /// 护栏：无法识别的行
    Odd { line: &'a [u8], line_no: u32 },
}

/// TIME 文本 → 批次号（minutes since 2000-01-01）。
pub fn batch_no_of(time: &[u8]) -> Option<i64> {
    let b = time;
    if b.len() != 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b' '
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let y = parse_u32(&b[0..4])?;
    let mo = parse_u32(&b[5..7])?;
    let d = parse_u32(&b[8..10])?;
    let h = parse_u32(&b[11..13])?;
    let mi = parse_u32(&b[14..16])?;
    parse_u32(&b[17..19])?; // 秒：不参与批次号，但要校验
    let days = days_from_civil(y as i64, mo as i64, d as i64) - days_from_civil(2000, 1, 1);
    Some(days * 1440 + h as i64 * 60 + mi as i64)
}

/// Howard Hinnant 的 civil-from-days（只用作日期差）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 事件流扫描器。零分配。
pub struct Scan<'a> {
    rest: &'a [u8],
    line_no: u32,
    time_seen: bool,
}

impl<'a> Scan<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Scan {
            rest: bytes,
            line_no: 0,
            time_seen: false,
        }
    }
}

/// 从 `USE ME:MEID=123;` 参数体中取 MEID。
pub fn meid_of(body: &[u8]) -> Option<u32> {
    first_param(body, b"MEID").and_then(parse_u32)
}

/// 在参数体中按名（大小写不敏感）取第一个值切片。
pub fn first_param<'b>(body: &'b [u8], name: &[u8]) -> Option<&'b [u8]> {
    Params::new(body)
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// 参数名哈希：与 `ims_mem2_macros::param_hash` **逐位一致**。
///
/// 宏在编译期给每张表建"槽位 → 参数下标"的静态表，运行期用它 O(1) 分派参数
/// （原来每个参数都要走一遍链式 `eq_ignore_ascii_case`，ASBR 是 18 个分支）。
#[inline]
pub fn param_hash(name: &[u8]) -> u32 {
    if name.is_empty() {
        return 0;
    }
    let n = name.len() as u32;
    let f = name[0].to_ascii_uppercase() as u32;
    let l = name[name.len() - 1].to_ascii_uppercase() as u32;
    n.wrapping_mul(0x9E37_79B1) ^ (f << 16) ^ (l << 3) ^ (n << 11)
}

/// 参数体迭代器：`K=V,K=V,...`（引号内逗号不分隔；K/V 已 trim）。
pub struct Params<'a> {
    rest: &'a [u8],
}

impl<'a> Params<'a> {
    pub fn new(body: &'a [u8]) -> Self {
        Params {
            rest: trim_ws(body),
        }
    }
}

impl<'a> Iterator for Params<'a> {
    type Item = (&'a [u8], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.rest.is_empty() {
                return None;
            }
            let mut in_quotes = false;
            let mut i = 0;
            while i < self.rest.len() {
                match self.rest[i] {
                    b'"' => in_quotes = !in_quotes,
                    b',' if !in_quotes => break,
                    _ => {}
                }
                i += 1;
            }
            let (kv, tail) = if i < self.rest.len() {
                (&self.rest[..i], &self.rest[i + 1..])
            } else {
                (self.rest, &self.rest[self.rest.len()..])
            };
            self.rest = tail;
            let kv = trim_ws(kv);
            if kv.is_empty() {
                continue;
            }
            let Some(eq) = kv.iter().position(|&c| c == b'=') else {
                continue;
            };
            let k = trim_ws(&kv[..eq]);
            let v = trim_ws(&kv[eq + 1..]);
            if k.is_empty() {
                continue;
            }
            return Some((k, v));
        }
    }
}

#[inline]
fn trim_ws(b: &[u8]) -> &[u8] {
    let mut s = 0;
    let mut e = b.len();
    while s < e && (b[s] == b' ' || b[s] == b'\t') {
        s += 1;
    }
    while e > s && (b[e - 1] == b' ' || b[e - 1] == b'\t') {
        e -= 1;
    }
    &b[s..e]
}

impl<'a> Iterator for Scan<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        loop {
            if self.rest.is_empty() {
                return None;
            }
            let (line, tail) = match self.rest.iter().position(|&c| c == b'\n') {
                Some(i) => (&self.rest[..i], &self.rest[i + 1..]),
                None => (self.rest, &self.rest[self.rest.len()..]),
            };
            self.rest = tail;
            self.line_no += 1;
            let line_no = self.line_no;
            let t = trim_ws(trim_eol(line));
            if t.is_empty() {
                continue;
            }
            if t.starts_with(b"/*") {
                if let Some(inner) = comment_inner(t) {
                    let inner = trim_ws(inner);
                    if !self.time_seen {
                        if let Some(time) = strip_prefix_ci(inner, b"TIME = ") {
                            if let Some(batch_no) = batch_no_of(trim_ws(time)) {
                                self.time_seen = true;
                                return Some(Event::Time {
                                    text: trim_ws(time),
                                    batch_no,
                                });
                            }
                        }
                    } else if strip_prefix_ci(inner, b"ME Type=").is_some() {
                        let (me_type, version) = split_me_header(inner);
                        return Some(Event::MeHeader { me_type, version });
                    }
                }
                continue; // 其它注释（含块头 `/* MOC = X */`）忽略
            }
            let semi = {
                let mut in_q = false;
                let mut found = None;
                for (i, &c) in t.iter().enumerate() {
                    match c {
                        b'"' => in_q = !in_q,
                        b';' if !in_q => {
                            found = Some(i);
                            break;
                        }
                        _ => {}
                    }
                }
                found
            };
            let Some(semi) = semi else {
                return Some(Event::Odd { line: t, line_no });
            };
            if !trim_ws(&t[semi + 1..]).is_empty() {
                return Some(Event::Odd { line: t, line_no });
            }
            let head = &t[..semi];
            let Some(colon) = head.iter().position(|&c| c == b':') else {
                return Some(Event::Odd { line: t, line_no });
            };
            let verb = trim_ws(&head[..colon]);
            let (op_name, object) = match verb.iter().position(|&c| c == b' ') {
                Some(sp) => (&verb[..sp], trim_ws(&verb[sp + 1..])),
                None => (verb, &verb[..0]),
            };
            if op_name.is_empty() || object.is_empty() {
                return Some(Event::Odd { line: t, line_no });
            }
            let op = Op::parse(op_name);
            let body = trim_ws(&head[colon + 1..]);
            if op == Op::Other
                && op_name.eq_ignore_ascii_case(b"USE")
                && object.eq_ignore_ascii_case(b"ME")
            {
                if let Some(meid) = meid_of(body) {
                    return Some(Event::Segment { meid });
                }
            }
            return Some(Event::Mml {
                op,
                object,
                body,
                line_no,
            });
        }
    }
}

#[inline]
fn trim_eol(b: &[u8]) -> &[u8] {
    let mut e = b.len();
    while e > 0 && b[e - 1] == b'\r' {
        e -= 1;
    }
    &b[..e]
}

fn comment_inner(t: &[u8]) -> Option<&[u8]> {
    if t.len() >= 4 && t.starts_with(b"/*") && t.ends_with(b"*/") {
        Some(&t[2..t.len() - 2])
    } else {
        None
    }
}

fn strip_prefix_ci<'b>(s: &'b [u8], p: &[u8]) -> Option<&'b [u8]> {
    if s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p) {
        Some(&s[p.len()..])
    } else {
        None
    }
}

/// `ME Type=CGP, Version=V100R007C00SPC200 PatchVersion=...` → (type, version)
fn split_me_header(inner: &[u8]) -> (&[u8], &[u8]) {
    let rest = strip_prefix_ci(inner, b"ME Type=").unwrap_or(inner);
    let (ty, ver) = match rest.iter().position(|&c| c == b',') {
        Some(i) => (&rest[..i], trim_ws(&rest[i + 1..])),
        None => (rest, &rest[rest.len()..]),
    };
    let ver = strip_prefix_ci(ver, b"Version=").unwrap_or(ver);
    (trim_ws(ty), trim_ws(ver))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &[u8] = b"/* TIME = 2026-09-21 03:30:30 */\r\n\
/* ME Type=CGP, Version=V100R007C00SPC200 PatchVersion=x */\r\n\
USE ME:MEID=0;\r\n\
SET NET:CASCADMODE=INTERNALBASEMODE;\r\n\
ADD ME:DMID=1,SNID=0,MEID=281,MENAME=\"AGCF63\",METYPE=AGCF;\r\n\
/* MOC = ASBR */\r\n\
ADD ASBR:PUI=\"xx@dd\",MN=1000,HNID=\"GDHNID\";\r\n\
rmv cnacld:LP=65534,PFX=K'140;\r\n\
/* ME Type=AGCF, Version=V100R003C50SPC100 */\r\n\
USE ME:MEID=281;\r\n\
ADD CNACLD:LP=0,LDN=\"IMS,to PBX\",PFX=K'0086755;\r\n\
garbage line without semicolon\r\n\
";

    #[test]
    fn events_stream() {
        let ev: Vec<Event> = Scan::new(MINI).collect();
        let mut it = ev.into_iter();
        match it.next().unwrap() {
            Event::Time { batch_no, .. } => assert_eq!(batch_no, 14054610),
            e => panic!("{e:?}"),
        }
        match it.next().unwrap() {
            Event::MeHeader { me_type, version } => {
                assert_eq!(me_type, b"CGP");
                assert!(version.starts_with(b"V100R007C00SPC200"));
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(it.next().unwrap(), Event::Segment { meid: 0 }));
        match it.next().unwrap() {
            Event::Mml { op, object, .. } => {
                assert_eq!(op, Op::Set);
                assert_eq!(object, b"NET");
            }
            e => panic!("{e:?}"),
        }
        match it.next().unwrap() {
            Event::Mml { op, object, body, .. } => {
                assert_eq!(op, Op::Add);
                assert_eq!(object, b"ME");
                assert_eq!(first_param(body, b"MENAME"), Some(&b"\"AGCF63\""[..]));
            }
            e => panic!("{e:?}"),
        }
        match it.next().unwrap() {
            Event::Mml { op, object, .. } => {
                assert_eq!(op, Op::Add);
                assert_eq!(object, b"ASBR");
            }
            e => panic!("{e:?}"),
        }
        match it.next().unwrap() {
            Event::Mml { op, object, body, .. } => {
                assert_eq!(op, Op::Rmv);
                assert_eq!(object, b"cnacld");
                assert!(first_param(body, b"pfx").is_some());
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(it.next().unwrap(), Event::MeHeader { me_type, .. } if me_type == b"AGCF"));
        assert!(matches!(it.next().unwrap(), Event::Segment { meid: 281 }));
        match it.next().unwrap() {
            Event::Mml { op, object, body, .. } => {
                assert_eq!(op, Op::Add);
                assert_eq!(object, b"CNACLD");
                let kv: Vec<_> = Params::new(body).collect();
                assert_eq!(kv.len(), 3);
                assert_eq!(kv[1].1, &b"\"IMS,to PBX\""[..], "引号内逗号不分隔");
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(it.next().unwrap(), Event::Odd { .. }));
        assert!(it.next().is_none());
    }

    #[test]
    fn params_iter_forms() {
        let body = b" PUI=\"a@b\" ,  MN=1000,,X=,=v,  LAST=1";
        let kv: Vec<_> = Params::new(body).collect();
        assert_eq!(kv.len(), 4);
        assert_eq!(kv[0], (&b"PUI"[..], &b"\"a@b\""[..]));
        assert_eq!(kv[1], (&b"MN"[..], &b"1000"[..]));
        assert_eq!(kv[2], (&b"X"[..], &b""[..]));
        assert_eq!(kv[3], (&b"LAST"[..], &b"1"[..]));
    }

    #[test]
    fn batch_no_math() {
        assert_eq!(batch_no_of(b"2023-09-19 03:30:48"), Some(12473490));
        assert_eq!(batch_no_of(b"2026-09-21 03:30:30"), Some(14054610));
        assert_eq!(batch_no_of(b"bad"), None);
    }

    #[test]
    fn meid_parse() {
        assert_eq!(meid_of(b"MEID=281"), Some(281));
        assert_eq!(meid_of(b"X=1,MEID=0"), Some(0));
        assert_eq!(meid_of(b"MEID=\"281\""), Some(281));
    }
}
