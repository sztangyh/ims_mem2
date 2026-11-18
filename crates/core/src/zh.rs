//! 中文词典：MOC / 参数 / 取值的中文名与解释（数据见 `zh.tsv`，生成器 `tools/gen_zh.py`）。
//!
//! 为什么单独一份而不是塞进 `Library::interner`：那个 arena 存的是**GBK 原字节**
//! （`resolve` 出来要 `decode()` 才成 UTF-8），而 `zh.tsv` 是 UTF-8 —— 混用会让
//! `decode()` 把中文再按 GBK 解一遍，直接乱码。所以这里**自带一个 UTF-8 的 `Interner`**，
//! 内存账见 `bytes()`。
//!
//! 四个层级（与 `zh.tsv` 的 kind 一一对应）：
//!
//! | kind | 键 | 用途 | 行数 |
//! |---|---|---|---|
//! | `M` | (MOC, OP) | 命令的中文名（`cmd_cn` / `name_cn`） | 503 |
//! | `P` | (MOC, PARAM) | **一级**参数中文名 | 5,416 |
//! | `Q` | PARAM | **二级**兜底（跨 MOC，带 `n_moc:ambiguous`） | 2,661 |
//! | `V` | (MOC, PARAM, VALUE) | 枚举取值的中文解释 | 4,206 |
//! | `W` | (PARAM, VALUE) | 取值的二级兜底 | 3,393 |
//!
//! 两级的必要性：同一个参数名（`TG`/`TGN`/`CSC`）在不同 MOC 下是一个意思，
//! 但 `zh_param` 是按 (MOC, PARAM) 记的，所以 `Trunk` 这种跨 15 张来源表拼出来的对象
//! 只能靠 `zh_param_any` 兜底 —— 实测能把覆盖率从 66.9% 抬到 **70.0%**。

use std::collections::HashMap;

use crate::sym::{Interner, Sym};

/// `zh.tsv` 的转义还原（与 `tools/gen_zh.py::esc` 互为逆；只认 `\\` `\t` `\n`）。
fn unescape(v: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len());
    let mut i = 0;
    while i < v.len() {
        if v[i] == b'\\' && i + 1 < v.len() {
            i += 1;
            out.push(match v[i] {
                b't' => b'\t',
                b'n' => b'\n',
                other => other,
            });
        } else {
            out.push(v[i]);
        }
        i += 1;
    }
    out
}

/// 中文词典。默认空（未装载时所有查询返回 `None`，调用方无需判空）。
#[derive(Debug, Default)]
pub struct ZhDict {
    /// 文本池，**UTF-8**（不是 GBK！见模块注释）
    it: Interner,
    moc: HashMap<(Sym, Sym), (Sym, Sym)>,
    param: HashMap<(Sym, Sym), Sym>,
    /// `param → (中文名, 出现过的 MOC 数, 是否撞名)`
    param_any: HashMap<Sym, (Sym, u16, bool)>,
    value: HashMap<(Sym, Sym, Sym), Sym>,
    /// `(param, value) → (中文解释, 出现过的 MOC 数, 是否撞名)`
    value_any: HashMap<(Sym, Sym), (Sym, u16, bool)>,
    /// 开关名（位名）：`(moc, param, switch) → (中文, 来源)`
    ///
    /// 来源 `0=manual`（人工裁决，最准）/ `1=doc`（文档自动解析）/ `2=vocab`
    /// （位词词典，**只在无歧义时**收录）。见 `tools/gen_zh.py` 文件头的三方对比。
    switch: HashMap<(Sym, Sym, Sym), (Sym, u8)>,
    /// 装载行数（报告用）
    rows: usize,
}

impl ZhDict {
    pub fn new() -> Self {
        Self::default()
    }

    /// 装载 `zh.tsv`（TAB 分隔 6 列；`#` 开头是注释）。返回行数。
    ///
    /// 坏行跳过并计数（不 panic）：字典是显示用的，缺几条不该让导入失败。
    pub fn load(&mut self, bytes: &[u8]) -> usize {
        let mut n = 0usize;
        for line in bytes.split(|b| *b == b'\n') {
            let line = if line.ends_with(b"\r") {
                &line[..line.len() - 1]
            } else {
                line
            };
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
            // 6 列；text 里可能有 TAB 的转义，但字段本身不含裸 TAB（生成器已转义）
            let mut f: [&[u8]; 6] = [b""; 6];
            let mut i = 0usize;
            let mut start = 0usize;
            for (pos, &c) in line.iter().enumerate() {
                if c == b'\t' {
                    if i < 6 {
                        f[i] = &line[start..pos];
                    }
                    i += 1;
                    start = pos + 1;
                }
            }
            if i < 6 {
                f[i] = &line[start..];
            }
            let k1 = unescape(f[1]);
            let k2 = unescape(f[2]);
            let k3 = unescape(f[3]);
            let text = unescape(f[4]);
            let extra = unescape(f[5]);
            let t = self.it.intern(&text);
            match f[0].first().copied() {
                Some(b'M') => {
                    // text = cmd_cn（第 5 列），extra = name_cn（第 6 列）
                    let name_cn = self.it.intern(&extra);
                    let m = self.it.intern(&k1);
                    let o = self.it.intern(&k2);
                    self.moc.insert((m, o), (t, name_cn));
                }
                Some(b'P') => {
                    let m = self.it.intern(&k1);
                    let p = self.it.intern(&k2);
                    self.param.insert((m, p), t);
                }
                Some(b'Q') => {
                    let p = self.it.intern(&k1);
                    let (n_moc, amb) = parse_meta(&extra);
                    self.param_any.insert(p, (t, n_moc, amb));
                }
                Some(b'V') => {
                    let m = self.it.intern(&k1);
                    let p = self.it.intern(&k2);
                    let v = self.it.intern(&k3);
                    self.value.insert((m, p, v), t);
                }
                Some(b'W') => {
                    let p = self.it.intern(&k1);
                    let v = self.it.intern(&k2);
                    let (n_moc, amb) = parse_meta(&extra);
                    self.value_any.insert((p, v), (t, n_moc, amb));
                }
                Some(b'S') => {
                    let m = self.it.intern(&k1);
                    let p = self.it.intern(&k2);
                    let n = self.it.intern(&k3);
                    // extra = 来源词（manual / doc / vocab）
                    let src = match extra.first().copied() {
                        Some(b'm') => 0u8,
                        Some(b'd') => 1,
                        _ => 2,
                    };
                    self.switch.insert((m, p, n), (t, src));
                }
                _ => continue,
            }
            n += 1;
        }
        self.rows = n;
        n
    }

    /// 装载行数。
    pub fn len(&self) -> usize {
        self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    #[inline]
    fn text(&self, s: Sym) -> &str {
        std::str::from_utf8(self.it.resolve(s)).unwrap_or("")
    }

    /// 键查表（**大小写不敏感**）：先按原文查，再按 ASCII 大写查。
    ///
    /// 为什么需要：`zh.tsv` 的键统一 `upper()`，但服务层的字段名是**小写别名**
    /// （`Trunk` 的 `tg`/`binifid`/`mgw_mid`，见 `svc/engine.rs::primary_specs`）——
    /// 大小写敏感会让 `Trunk` 的字段一个中文名都查不到（实测踩过）。
    fn key_ci(&self, s: &str) -> Option<Sym> {
        if let Some(x) = self.it.lookup(s.as_bytes()) {
            return Some(x);
        }
        if s.is_ascii() && s.len() <= 64 {
            let mut buf = [0u8; 64];
            for (i, b) in s.bytes().enumerate() {
                buf[i] = b.to_ascii_uppercase();
            }
            return self.it.lookup(&buf[..s.len()]);
        }
        self.it.lookup(s.to_uppercase().as_bytes())
    }

    /// 参数中文名：先按 `(MOC, PARAM)`，再按 `PARAM` 兜底。**MOC/PARAM 大小写不敏感**。
    pub fn param_cn(&self, moc: &str, param: &str) -> Option<&str> {
        let p = self.key_ci(param)?;
        if !moc.is_empty() {
            if let Some(m) = self.key_ci(moc) {
                if let Some(&t) = self.param.get(&(m, p)) {
                    return Some(self.text(t));
                }
            }
        }
        self.param_any.get(&p).map(|(t, _, _)| self.text(*t))
    }

    /// 二级兜底的元信息 `(出现过的 MOC 数, 是否撞名)`；`ambiguous=false` 才敢无条件展示。
    pub fn param_any_meta(&self, param: &str) -> Option<(u16, bool)> {
        let p = self.key_ci(param)?;
        self.param_any.get(&p).map(|(_, n, a)| (*n, *a))
    }

    /// 枚举取值的中文解释：先 `(MOC, PARAM, VALUE)`，再 `(PARAM, VALUE)` 兜底。
    ///
    /// `moc`/`param` 大小写不敏感；`value` **保持大小写敏感**（枚举词本身有大小写语义，
    /// 如 `ESL` 与 `esl` 不是一回事，归一化会张冠李戴）。
    pub fn value_cn(&self, moc: &str, param: &str, value: &str) -> Option<&str> {
        let p = self.key_ci(param)?;
        let v = self.it.lookup(value.as_bytes())?;
        if !moc.is_empty() {
            if let Some(m) = self.key_ci(moc) {
                if let Some(&t) = self.value.get(&(m, p, v)) {
                    return Some(self.text(t));
                }
            }
        }
        self.value_any.get(&(p, v)).map(|(t, _, _)| self.text(*t))
    }

    /// 命令的中文名 `(cmd_cn, name_cn)`。大小写不敏感。
    pub fn cmd_cn(&self, moc: &str, op: &str) -> Option<(&str, &str)> {
        let m = self.key_ci(moc)?;
        let o = self.key_ci(op)?;
        let (a, b) = self.moc.get(&(m, o))?;
        Some((self.text(*a), self.text(*b)))
    }

    /// **开关名（位名）的中文说明** `(说明, 来源)`。
    ///
    /// 必须按 `(MOC, PARAM, 开关名)` **三元组**精确查：开关名是通用短词
    /// （`SVR2`/`AEC`/`TIN`），同一个词在不同参数下含义不同 —— 实测 1,058 个开关名里
    /// **521 个是"跨多个位序空间"的多义词**，按词查会把 `APRATGDEF.SVRCTRL.SVR2`
    /// （出中继Setup消息传输能力强制置 speech）答成 `N1TGDEF` 的"接续成功时由中继送回铃音"。
    ///
    /// 来源：`"manual"`（人工裁决）/ `"doc"`（文档解析）/ `"vocab"`（位词词典，仅无歧义）。
    pub fn switch_cn(&self, moc: &str, param: &str, name: &str) -> Option<(&str, &'static str)> {
        let m = self.key_ci(moc)?;
        let p = self.key_ci(param)?;
        let n = self.key_ci(name)?;
        let (t, src) = self.switch.get(&(m, p, n))?;
        let s = match src {
            0 => "manual",
            1 => "doc",
            _ => "vocab",
        };
        Some((self.text(*t), s))
    }

    /// `S` 层：`(moc, param, switch, 中文, 来源)`。
    pub fn iter_switch(&self) -> impl Iterator<Item = (&str, &str, &str, &str, &'static str)> + '_ {
        self.switch.iter().map(|((m, p, n), (t, s))| {
            (
                self.text(*m),
                self.text(*p),
                self.text(*n),
                self.text(*t),
                match s {
                    0 => "manual",
                    1 => "doc",
                    _ => "vocab",
                },
            )
        })
    }

    // ---- 遍历（投影成 `zh_*` 表用；与 ims_core 的 5 张表同名同构）----

    /// `M` 层：`(moc, op, cmd_cn, name_cn)`。
    pub fn iter_moc(&self) -> impl Iterator<Item = (&str, &str, &str, &str)> + '_ {
        self.moc
            .iter()
            .map(|((m, o), (a, b))| (self.text(*m), self.text(*o), self.text(*a), self.text(*b)))
    }

    /// `P` 层：`(moc, param, param_cn)`。
    pub fn iter_param(&self) -> impl Iterator<Item = (&str, &str, &str)> + '_ {
        self.param
            .iter()
            .map(|((m, p), t)| (self.text(*m), self.text(*p), self.text(*t)))
    }

    /// `Q` 层：`(param, param_cn, n_moc, ambiguous)`。
    pub fn iter_param_any(&self) -> impl Iterator<Item = (&str, &str, u16, bool)> + '_ {
        self.param_any
            .iter()
            .map(|(p, (t, n, a))| (self.text(*p), self.text(*t), *n, *a))
    }

    /// `V` 层：`(moc, param, value, value_cn)`。
    pub fn iter_value(&self) -> impl Iterator<Item = (&str, &str, &str, &str)> + '_ {
        self.value
            .iter()
            .map(|((m, p, v), t)| {
                (self.text(*m), self.text(*p), self.text(*v), self.text(*t))
            })
    }

    /// `W` 层：`(param, value, value_cn, n_moc, ambiguous)`。
    pub fn iter_value_any(&self) -> impl Iterator<Item = (&str, &str, &str, u16, bool)> + '_ {
        self.value_any
            .iter()
            .map(|((p, v), (t, n, a))| (self.text(*p), self.text(*v), self.text(*t), *n, *a))
    }

    /// 自身占用（报告用）：文本池 + 5 张表。
    pub fn bytes(&self) -> usize {
        let entries = self.moc.len()
            + self.param.len()
            + self.param_any.len()
            + self.value.len()
            + self.value_any.len()
            + self.switch.len();
        self.it.storage_bytes()
            + self.it.index_bytes()
            // 每条约 40 B（键 4~12 B + HashMap 桶/控制字节 ~28 B，按最保守估）
            + entries * 40
    }
}

/// `n_moc:ambiguous` → `(u16, bool)`；解析不出来就按 `(0, true)`（保守：当撞名）。
fn parse_meta(b: &[u8]) -> (u16, bool) {
    let mut it = b.split(|c| *c == b':');
    let n = it.next().and_then(|x| std::str::from_utf8(x).ok()).and_then(|x| x.parse::<u16>().ok());
    let a = it.next().and_then(|x| std::str::from_utf8(x).ok()).and_then(|x| x.parse::<u16>().ok());
    match (n, a) {
        (Some(n), Some(a)) => (n, a != 0),
        _ => (0, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 6 列：kind / k1 / k2 / k3 / text / extra（与 `tools/gen_zh.py` 的输出同形）
    const SAMPLE: &[u8] = b"# test\n\
M\tAGCF\tADD\t\t\xe6\xb7\xbb\xe5\x8a\xa0\xe6\x9c\xac\xe5\xb1\x80\t\xe6\xb7\xbb\xe5\x8a\xa0\n\
P\tPRATG\tCDEF\t\t\xe7\xbc\xba\xe7\x9c\x81\xe4\xb8\xbb\xe5\x8f\xab\t\n\
Q\tTG\t\t\t\xe4\xb8\xad\xe7\xbb\xa7\t3:1\n\
V\tASBR\tDID\tESL\tESL\xe7\x94\xa8\xe6\x88\xb7\t\n\
W\tDID\tPRA\t\tPRA\xe7\x94\xa8\xe6\x88\xb7\t2:0\n\
S\tMGW\tUCATT\tNOEC\t\xe4\xb8\x8d\xe5\x8c\x85\xe5\x90\xab\xe5\x9b\x9e\xe5\xa3\xb0\xe6\xb6\x88\xe9\x99\xa4\xe5\xad\x97\xe6\xae\xb5\tmanual\n\
S\tMGW\tUCATT\tNOFX\t\xe4\xb8\x8d\xe5\x90\xaf\xe7\x94\xa8\xe4\xbc\xa0\xe7\x9c\x9f\xe6\xa3\x80\xe6\xb5\x8b\tvocab\n";

    #[test]
    fn loads_all_five_tiers() {
        let mut z = ZhDict::new();
        let n = z.load(SAMPLE);
        assert_eq!(n, 7, "5 层 + 2 条开关名");
        assert_eq!(z.len(), 7);
    }

    #[test]
    fn param_lookup_is_two_tier() {
        let mut z = ZhDict::new();
        z.load(SAMPLE);
        // 一级：按 (MOC, PARAM)
        assert_eq!(z.param_cn("PRATG", "CDEF"), Some("缺省主叫"));
        // 二级：MOC 不对 → 退到 PARAM
        assert_eq!(z.param_cn("N7TG", "TG"), Some("中继"));
        // 空 MOC 也走二级
        assert_eq!(z.param_cn("", "TG"), Some("中继"));
        // 查不到 → None（不 panic）
        assert_eq!(z.param_cn("PRATG", "NOSUCH"), None);
        // 大小写不敏感：服务层的字段名是小写别名（`tg`/`binifid`），必须能查到
        assert_eq!(z.param_cn("PRATG", "cdef"), Some("缺省主叫"));
        assert_eq!(z.param_cn("pratg", "CDEF"), Some("缺省主叫"));
        assert_eq!(z.param_cn("N7TG", "tg"), Some("中继"));
    }

    #[test]
    fn param_any_meta_exposes_ambiguity() {
        let mut z = ZhDict::new();
        z.load(SAMPLE);
        assert_eq!(z.param_any_meta("TG"), Some((3, true)));
        assert_eq!(z.param_any_meta("NOSUCH"), None);
        // `Q` 行格式坏掉时按"撞名"处理，宁可少用中文名也不给错
        let mut z2 = ZhDict::new();
        z2.load(b"Q\tTG\t\xe4\xb8\xad\xe7\xbb\xa7\tbad\n");
        assert_eq!(z2.param_any_meta("TG"), Some((0, true)));
    }

    #[test]
    fn value_lookup_is_two_tier() {
        let mut z = ZhDict::new();
        z.load(SAMPLE);
        assert_eq!(z.value_cn("ASBR", "DID", "ESL"), Some("ESL用户"));
        // 二级兜底（MOC 不同）
        assert_eq!(z.value_cn("ASBR2", "DID", "PRA"), Some("PRA用户"));
        assert_eq!(z.value_cn("ASBR", "DID", "NOPE"), None);
    }

    #[test]
    fn cmd_lookup() {
        let mut z = ZhDict::new();
        z.load(SAMPLE);
        let (cmd, name) = z.cmd_cn("AGCF", "ADD").unwrap();
        assert!(cmd.contains("本局"));
        assert_eq!(name, "添加");
        assert!(z.cmd_cn("AGCF", "MOD").is_none());
    }

    #[test]
    fn switch_lookup_is_triplet_scoped() {
        let mut z = ZhDict::new();
        z.load(SAMPLE);
        // 三元组精确命中，并带出来源
        assert_eq!(z.switch_cn("MGW", "UCATT", "NOEC"),
                   Some(("不包含回声消除字段", "manual")));
        // 同一个开关名挂到别的参数上 → **查不到**（不能按词套用）
        assert_eq!(z.switch_cn("N7TG", "SVRCTRL", "NOEC"), None);
        assert_eq!(z.switch_cn("MGW", "UCATT", "NOSUCH"), None);
    }

    #[test]
    fn iterators_cover_all_tiers() {
        let mut z = ZhDict::new();
        z.load(SAMPLE);
        assert_eq!(z.iter_moc().count(), 1);
        assert_eq!(z.iter_param().count(), 1);
        assert_eq!(z.iter_param_any().count(), 1);
        assert_eq!(z.iter_value().count(), 1);
        assert_eq!(z.iter_value_any().count(), 1);
        assert_eq!(z.iter_switch().count(), 2);
    }

    #[test]
    fn escapes_and_crlf_and_bad_rows_are_tolerated() {
        let mut z = ZhDict::new();
        // CRLF + 转义 TAB + 字段数不足的坏行
        let n = z.load(b"P\tM\tP\t\tA\\tB\t\r\nXX\r\n");
        assert_eq!(n, 1, "坏行被跳过，不影响好行");
        assert_eq!(z.param_cn("M", "P"), Some("A\tB"));
    }
}

