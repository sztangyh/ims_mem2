//! 极小字节解析集（P0.2 只够 DNC 用；P0.3 再补 `Pfx/Uri/Eid/Bits` 等类型）。
//!
//! 原则与 ims_mem 一致：**不转码、不分配**，直接在 GBK 原字节上切/parse。

/// 去掉一层双引号（`"abc"` → `abc`）；无引号原样返回。
#[inline]
pub fn unquote(v: &[u8]) -> &[u8] {
    if v.len() >= 2 && v[0] == b'"' && v[v.len() - 1] == b'"' {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// 解析无符号整数（容忍外层引号）。空串/非法/溢出 → `None`。
///
/// 刻意不接收前导 `+`/空白：MML 数值参数实测是纯数字（`MEID="281"` 这种带引号的也通吃）。
pub fn parse_u32(v: &[u8]) -> Option<u32> {
    let v = unquote(v);
    if v.is_empty() || v.len() > 10 {
        return None;
    }
    let mut n: u64 = 0;
    for &c in v {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n * 10 + (c - b'0') as u64;
    }
    u32::try_from(n).ok()
}

#[inline]
pub fn parse_u16(v: &[u8]) -> Option<u16> {
    parse_u32(v).and_then(|n| u16::try_from(n).ok())
}

#[inline]
pub fn parse_u8(v: &[u8]) -> Option<u8> {
    parse_u32(v).and_then(|n| u8::try_from(n).ok())
}

/// MML 布尔四词法（`YES/NO/TRUE/FALSE`，另兼容 `1/0`）。
pub fn parse_bool(v: &[u8]) -> Option<bool> {
    let v = unquote(v);
    if v.eq_ignore_ascii_case(b"YES") || v.eq_ignore_ascii_case(b"TRUE") || v == b"1" {
        Some(true)
    } else if v.eq_ignore_ascii_case(b"NO") || v.eq_ignore_ascii_case(b"FALSE") || v == b"0" {
        Some(false)
    } else {
        None
    }
}

// ---------- 字冠（Pfx） ----------

/// 字冠 `K'...` 的最大半字节位数（文档上限 32 位）。
pub const PFX_MAX_DIGITS: usize = 32;

/// 字冠号码：**与 `mml_def::U4Number<17>` 同布局**（16 字节左对齐 BCD + 1 字节长度）。
///
/// 为什么照抄这个布局：`Ord`/`Eq` 是派生在 `[u8; 17]` 上的，
/// 而最后一字节是长度 —— 于是 **长度参与相等判定**（`14` ≠ `140`），
/// 这与 ims_mem 的引用键口径完全一致；换个布局就会在键上分叉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PfxNum([u8; 17]);

impl PfxNum {
    /// 由半字节序列构造（每元素 0..=15）。
    pub fn from_digits(digits: &[u8]) -> Option<PfxNum> {
        if digits.is_empty() || digits.len() > PFX_MAX_DIGITS {
            return None;
        }
        let mut b = [0u8; 17];
        let mut i = 0;
        while i < digits.len() {
            let hi = digits[i];
            let lo = if i + 1 < digits.len() { digits[i + 1] } else { 0 };
            if hi > 15 || lo > 15 {
                return None;
            }
            b[i / 2] = (hi << 4) | lo;
            i += 2;
        }
        b[16] = digits.len() as u8;
        Some(PfxNum(b))
    }

    /// 半字节位数。
    #[inline]
    pub fn len(&self) -> usize {
        self.0[16] as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 半字节序列（`0..=15`，从高位到低位）。
    pub fn digits(&self) -> impl Iterator<Item = u8> + '_ {
        let n = self.len();
        (0..n).map(move |i| {
            let byte = self.0[i / 2];
            if i % 2 == 0 {
                byte >> 4
            } else {
                byte & 0x0F
            }
        })
    }
}

/// 解析字冠：接受 `K'...`（大小写）或裸数字串；字符合法集 `0-9 / A-E / a-e / * / #`。
/// 由半字节序列（0..=15）构造 `PfxNum`；空序列/超过 32 位返回 `None`。
pub fn pfx_from_digits(d: &[u8]) -> Option<PfxNum> {
    PfxNum::from_digits(d)
}

pub fn parse_pfx(v: &[u8]) -> Option<PfxNum> {
    let v = unquote(v);
    let q = if v.len() >= 2 && (v[0] == b'K' || v[0] == b'k') && v[1] == b'\'' {
        &v[2..]
    } else {
        v
    };
    if q.is_empty() || q.len() > PFX_MAX_DIGITS {
        return None;
    }
    let mut digits = [0u8; PFX_MAX_DIGITS];
    for (i, &c) in q.iter().enumerate() {
        digits[i] = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'E' => c - b'A' + 10,
            b'a'..=b'e' => c - b'a' + 10,
            b'*' => 14,
            b'#' => 15,
            _ => return None,
        };
    }
    PfxNum::from_digits(&digits[..q.len()])
}


/// A2：规范形的无符号十进制（非空、纯数字、无前导零、≤ max）。
///
/// 为什么这么严：A2 的口径是"typed 值渲染回去必须**逐字节等于原文**"，
/// 所以只接受规范形；`010`/`+1`/空串一律返回 None → 退回原文（D52 的例外原样）。
fn canon_u32(v: &[u8], max: u32) -> Option<u32> {
    if v.is_empty() || v.len() > 10 {
        return None;
    }
    if v.len() > 1 && v[0] == b'0' {
        return None;
    }
    let mut n: u64 = 0;
    for &c in v {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n * 10 + (c - b'0') as u64;
    }
    if n > max as u64 {
        None
    } else {
        Some(n as u32)
    }
}

// ---------- A2：宽值类型（Ip / Eid / Bits / Pair / Spc） ----------
//
// 行内只占 4 B（`ColVal::group(池下标)`，见 D58），池里放 u64。
// **只有"渲染回去 == 原文"的值才走 typed**，否则退回原文（`ColVal::sym`）并计数；
// 实测例外：Bits 74 个（MGW.UCATT 40 / MGW.LISTOFCODEC 34 的稀疏写法）、Ip/Eid 6 个。

/// IPv4 → u32。规范形：四段十进制、无前导零、每段 ≤255。
pub fn parse_ip(v: &[u8]) -> Option<u32> {
    let v = unquote(v);
    let mut out = 0u32;
    let mut it = v.split(|&c| c == b'.');
    for _ in 0..4 {
        let seg = it.next()?;
        let n = canon_u32(seg, 255)?;
        out = (out << 8) | n;
    }
    if it.next().is_some() {
        return None;
    }
    Some(out)
}

pub fn render_ip(v: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        (v >> 24) & 0xff,
        (v >> 16) & 0xff,
        (v >> 8) & 0xff,
        v & 0xff
    )
}

/// Eid（`IP:port`）→ u64：`ip << 16 | port`。规范形：IPv4 规范 + 端口十进制无前导零。
pub fn parse_eid(v: &[u8]) -> Option<u64> {
    let v = unquote(v);
    let at = v.iter().position(|&c| c == b':')?;
    let ip = parse_ip(&v[..at])?;
    let port = canon_u32(&v[at + 1..], 65535)?;
    Some(((ip as u64) << 16) | port as u64)
}

pub fn render_eid(v: u64) -> String {
    format!("{}:{}", render_ip((v >> 16) as u32), v & 0xffff)
}

/// 位串 → u64：按位名表把 `NAME-0/1`（裸 `NAME` 视为 0）打到位上。
///
/// 位名表外出现名字、`-` 后不是 0/1、位名超过 64 个 → `None`（走原文兜底）。
pub fn parse_bits(v: &[u8], names: &[&str]) -> Option<u128> {
    let v = unquote(v);
    if v.is_empty() || names.len() > 128 {
        return None;
    }
    let mut bits = 0u128;
    for seg in v.split(|&c| c == b'&') {
        if seg.is_empty() {
            continue;
        }
        let (name, on) = match seg.iter().position(|&c| c == b'-') {
            Some(i) => (&seg[..i], &seg[i + 1..]),
            None => (seg, b"0".as_slice()),
        };
        let bit = names
            .iter()
            .position(|n| n.as_bytes().eq_ignore_ascii_case(name))?;
        match on {
            b"1" => bits |= 1 << bit,
            b"0" => {}
            _ => return None,
        }
    }
    Some(bits)
}

/// 位串回显：**按位名表序全列** `NAME-0/1`（规范形；与原文的书写顺序无关）。
pub fn render_bits(bits: u128, names: &[&str]) -> String {
    let mut s = String::with_capacity(names.len() * 10);
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            s.push('&');
        }
        s.push_str(n);
        s.push('-');
        s.push(if bits >> i & 1 == 1 { '1' } else { '0' });
    }
    s
}

/// `a-b` 值对 → u64：`a << 32 | b`（两边都是规范无符号十进制）。
pub fn parse_pair(v: &[u8]) -> Option<u64> {
    let v = unquote(v);
    let dash = v.iter().position(|&c| c == b'-')?;
    let a = canon_u32(&v[..dash], u32::MAX)?;
    let b = canon_u32(&v[dash + 1..], u32::MAX)?;
    Some(((a as u64) << 32) | b as u64)
}

pub fn render_pair(v: u64) -> String {
    format!("{}-{}", v >> 32, v & 0xffff_ffff)
}

/// `Spc`：6 位十六进制信令点码 → u32（24 bit）。
pub fn parse_spc(v: &[u8]) -> Option<u32> {
    let v = unquote(v);
    if v.len() != 6 || !v.iter().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(v).ok()?, 16).ok()
}

pub fn render_spc(v: u32) -> String {
    format!("{v:06X}")
}

/// A3：`PfxNum` 的**规范文本**（`K'` + 大写十六进制数字；14→`E`、15→`F`）。
///
/// 这是伴生列 `<NAME>_NUM` 的渲染口径 —— 与原文列无关（原文列保持原样）。
pub fn render_pfx(n: PfxNum) -> String {
    let mut s = String::with_capacity(2 + n.len());
    s.push_str("K'");
    for d in n.digits() {
        s.push(char::from_digit(d as u32, 16).unwrap_or('?').to_ascii_uppercase());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers() {
        assert_eq!(parse_u32(b"281"), Some(281));
        assert_eq!(parse_u32(b"\"281\""), Some(281));
        assert_eq!(parse_u32(b""), None);
        assert_eq!(parse_u32(b"12a"), None);
        assert_eq!(parse_u32(b"-1"), None);
        assert_eq!(parse_u16(b"65535"), Some(65535));
        assert_eq!(parse_u16(b"65536"), None);
        assert_eq!(parse_u8(b"255"), Some(255));
        assert_eq!(parse_u8(b"256"), None);
        assert_eq!(parse_u32(b"4294967296"), None);
    }

    #[test]
    fn ip_roundtrip_is_byte_exact_or_rejected() {
        assert_eq!(parse_ip(b"10.1.2.3").map(render_ip).as_deref(), Some("10.1.2.3"));
        assert_eq!(parse_ip(b"\"255.255.255.255\"").map(render_ip).as_deref(), Some("255.255.255.255"));
        // 非规范形：一律拒绝（走原文兜底，保证"渲染 == 原文"）
        for bad in [b"010.1.2.3".as_slice(), b"10.1.2".as_slice(), b"10.1.2.3.4", b"10.1.2.256", b"1.2.3.", b""] {
            assert_eq!(parse_ip(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn eid_roundtrip_and_rejects() {
        assert_eq!(parse_eid(b"10.45.41.74:2944").map(render_eid).as_deref(), Some("10.45.41.74:2944"));
        // 实测里真实出现的 6 个坏形状 → 必须拒绝
        for bad in [
            b"cdma_ss6.com".as_slice(),
            b"10.46.56.199.251:2944",
            b"10.45.41.74\xa3\xba2944", // 全角冒号
            b"5.154.78.3:2944\t",
            b"10.1.2.3:02944",
            b"10.1.2.3:",
        ] {
            assert_eq!(parse_eid(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn bits_roundtrip_is_canonical_only() {
        let names: &[&str] = &["CLEARMODE", "PCMU", "G729"];
        // 规范形（全列、按表序）→ typed
        let canon = b"CLEARMODE-1&PCMU-0&G729-1";
        let v = parse_bits(canon, names).unwrap();
        assert_eq!(render_bits(v, names).as_bytes(), canon);
        // 稀疏写法（实测 74 个）→ 规范形与原文不同 → 必须退回原文
        let sparse = b"CLEARMODE-1";
        let v2 = parse_bits(sparse, names).unwrap();
        assert_ne!(render_bits(v2, names).as_bytes(), sparse);
        // 位名表外 / 非法位值 → None
        assert_eq!(parse_bits(b"UNKNOWN-1", names), None);
        assert_eq!(parse_bits(b"PCMU-2", names), None);
        // 裸名字 = 0
        assert_eq!(parse_bits(b"PCMU", names), Some(0));
    }

    #[test]
    fn pair_and_spc() {
        assert_eq!(parse_pair(b"3-7").map(render_pair).as_deref(), Some("3-7"));
        assert_eq!(parse_pair(b"01-7"), None);
        assert_eq!(parse_pair(b"4294967295-0").map(render_pair).as_deref(), Some("4294967295-0"));
        assert_eq!(parse_spc(b"00FF10").map(render_spc).as_deref(), Some("00FF10"));
        assert_eq!(parse_spc(b"00ff10").map(render_spc).as_deref(), Some("00FF10")); // 小写能解析但渲染是大写
        assert_eq!(parse_spc(b"00FF1"), None);
    }

    #[test]
    fn quotes() {
        assert_eq!(unquote(b"\"a,b\""), b"a,b");
        assert_eq!(unquote(b"a"), b"a");
        assert_eq!(unquote(b"\""), b"\"");
    }

    #[test]
    fn booleans() {
        assert_eq!(parse_bool(b"YES"), Some(true));
        assert_eq!(parse_bool(b"no"), Some(false));
        assert_eq!(parse_bool(b"TRUE"), Some(true));
        assert_eq!(parse_bool(b"FALSE"), Some(false));
        assert_eq!(parse_bool(b"\"YES\""), Some(true));
        assert_eq!(parse_bool(b"1"), Some(true));
        assert_eq!(parse_bool(b"0"), Some(false));
        assert_eq!(parse_bool(b"MAYBE"), None);
    }

    #[test]
    fn pfx_parse_and_order() {
        let a = parse_pfx(b"K'140").unwrap();
        let b = parse_pfx(b"140").unwrap();
        assert_eq!(a, b, "K' 前缀不影响数值");
        assert_eq!(a.len(), 3);
        assert_eq!(a.digits().collect::<Vec<_>>(), vec![1, 4, 0]);

        // 长度参与相等判定（与 mml_def::U4Number 同布局）：0140 ≠ 140
        let c = parse_pfx(b"0140").unwrap();
        assert_ne!(a, c);
        // 左对齐 BCD：首半字节 0x01 < 0x14，所以 0140 排在 140 前面（这是布局副作用，不是数值序）
        assert!(c < a, "0140 应排在 140 之前（左对齐比较首字节）");

        // 字符合法集
        assert!(parse_pfx(b"K'EEEE").is_some());
        assert!(parse_pfx(b"65534#").is_some());
        assert!(parse_pfx(b"K'").is_none(), "空字冠非法");
        assert!(parse_pfx(b"K'ZZZ").is_none(), "非十六进制字符非法");
        assert_eq!(parse_pfx(b"K'AAAA").unwrap().len(), 4);
    }

    /// 关键性质：**前导零必须保真** —— 拨号字冠 `0086755` 与 `86755` 是两个不同字冠。
    #[test]
    fn pfx_leading_zero_is_significant() {
        let a = parse_pfx(b"K'0086755").unwrap();
        let b = parse_pfx(b"K'86755").unwrap();
        assert_ne!(a, b, "前导零参与键相等判定");
        assert_eq!(a.len(), 7);
        assert_eq!(b.len(), 5);
        assert_eq!(a.digits().collect::<Vec<_>>(), vec![0, 0, 8, 6, 7, 5, 5]);

        // 若按"数值化 u32"存，两者都会变成 86755，键就撞了 —— 这正是不能用 Val32 的原因
        let as_u32_a: u32 = a.digits().fold(0u32, |acc, d| acc * 16 + d as u32);
        let as_u32_b: u32 = b.digits().fold(0u32, |acc, d| acc * 16 + d as u32);
        assert_eq!(as_u32_a, as_u32_b, "数值化会丢失前导零（所以不能用 Val32 存 Pfx）");
        assert_ne!(a, b, "但 PfxNum 用长度区分");
    }
}
