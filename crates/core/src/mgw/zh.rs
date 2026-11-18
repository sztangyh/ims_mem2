//! MGW 中文词典（`mgw_zh.tsv`）：对象名 + 参数名 + 参数说明。
//!
//! **与 AGCF 的 [`crate::zh::ZhDict`] 完全分开**，不是重复代码而是必要隔离：
//! AGCF 与 MGW 的参数**同名不同义**——`MID` 在 AGCF 是「ACU模块号」、在 MGW 是
//! 「虚拟媒体网关标识」；`BT`/`BN` 这类短名两边都有。合成一份就得靠 `moc` 区分，
//! 而两边又有 7 个同名 MOC（`BRD`/`IPADDR`/`N7DSP`/`N7LKS`/`N7LNK`/`N7RT`/`OFI`）。
//! 分开存、分开查，从结构上不可能串味。
//!
//! 规模：59 对象 / 712 参数（699 条有说明）；对本仓 43 张表的 **188 列 100% 覆盖**。
//! 文本用**自己的 UTF-8 池**（`String`），不碰 AGCF 的 GBK `Interner`。

use std::collections::HashMap;

/// MGW 中文词典。
#[derive(Debug, Default)]
pub struct MgwZh {
    /// `(MOC 大写, 参数大写) → (中文名, 说明)`
    params: HashMap<(String, String), (String, String)>,
    /// `MOC 大写 → (对象中文名, 标题)`
    mocs: HashMap<String, (String, String)>,
    bytes: usize,
}

impl MgwZh {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `mgw_zh.tsv` 装载。返回装载的条目数（0 = 空文件/只有注释）。
    pub fn load(&mut self, data: &[u8]) -> usize {
        self.bytes = data.len();
        let text = String::from_utf8_lossy(data);
        let mut n = 0;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 5 {
                continue;
            }
            let moc = unesc(f[1]).to_ascii_uppercase();
            match f[0] {
                "M" => {
                    self.mocs.insert(moc, (unesc(f[3]), unesc(f[4])));
                    n += 1;
                }
                "P" => {
                    let par = unesc(f[2]).to_ascii_uppercase();
                    self.params.insert((moc, par), (unesc(f[3]), unesc(f[4])));
                    n += 1;
                }
                _ => {}
            }
        }
        n
    }

    /// 参数中文名。
    pub fn param_cn(&self, moc: &str, param: &str) -> Option<&str> {
        let k = (moc.to_ascii_uppercase(), param.to_ascii_uppercase());
        self.params.get(&k).map(|x| x.0.as_str()).filter(|s| !s.is_empty())
    }

    /// 参数说明（文档原文，可能很长；699 条有值）。
    pub fn param_desc(&self, moc: &str, param: &str) -> Option<&str> {
        let k = (moc.to_ascii_uppercase(), param.to_ascii_uppercase());
        self.params.get(&k).map(|x| x.1.as_str()).filter(|s| !s.is_empty())
    }

    /// 对象中文名（如 `ATONE` → 「增加异步音」）。
    pub fn moc_cn(&self, moc: &str) -> Option<&str> {
        self.mocs
            .get(&moc.to_ascii_uppercase())
            .map(|x| x.0.as_str())
            .filter(|s| !s.is_empty())
    }

    /// 对象标题（如 `ATONE` → 「增加异步音(ADD ATONE)」）。
    pub fn moc_title(&self, moc: &str) -> Option<&str> {
        self.mocs
            .get(&moc.to_ascii_uppercase())
            .map(|x| x.1.as_str())
            .filter(|s| !s.is_empty())
    }

    pub fn param_count(&self) -> usize {
        self.params.len()
    }

    pub fn moc_count(&self) -> usize {
        self.mocs.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// 与 `tools/gen_mgw_zh.py` 的 `unesc` 对称。
fn unesc(v: &str) -> String {
    if !v.contains('\\') {
        return v.to_string();
    }
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b't' => {
                    out.push(b'\t');
                    i += 2;
                    continue;
                }
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                    continue;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_looks_up() {
        // 用数组 join 构造，避免 Rust 的 `\` 行续接把制表符/换行吃掉（被这个坑过一次）
        let tsv = [
            "# c",
            "M\tATONE\t\t增加异步音\t增加异步音(ADD ATONE)",
            "P\tATONE\tBT\t板类型\t必选参数。",
            "P\tVMGW\tMID\t虚拟媒体网关标识\t",
        ]
        .join("\n");
        let mut z = MgwZh::new();
        assert_eq!(z.load(tsv.as_bytes()), 3, "3 行数据（1 对象 + 2 参数）");
        assert_eq!(z.moc_cn("atone"), Some("增加异步音"));
        assert_eq!(z.moc_title("ATONE"), Some("增加异步音(ADD ATONE)"));
        assert_eq!(z.param_cn("ATONE", "bt"), Some("板类型"));
        assert_eq!(z.param_cn("VMGW", "MID"), Some("虚拟媒体网关标识"));
        assert_eq!(z.param_desc("ATONE", "BT"), Some("必选参数。"));
        // 三元组精确：换个 MOC 就查不到（这是防串味的关键）
        assert_eq!(z.param_cn("BRD", "BT"), None);
        assert_eq!(z.param_desc("VMGW", "MID"), None, "空说明返回 None");
        assert_eq!(z.param_count(), 2);
        assert_eq!(z.moc_count(), 1);
    }

    #[test]
    fn unescapes_newline_in_desc() {
        let tsv = "P\tA\tB\t中文\t第一行\\n第二行\n";
        let mut z = MgwZh::new();
        z.load(tsv.as_bytes());
        assert_eq!(z.param_desc("A", "B"), Some("第一行\n第二行"));
    }
}
