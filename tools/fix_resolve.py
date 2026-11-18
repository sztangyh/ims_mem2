# -*- coding: utf-8 -*-
"""修正 resolve/Diff 的签名与调用点（patch_table_diff.py 的补丁）。"""
import io
import re
import sys

PATH = 'crates/core/src/table.rs'
src = io.open(PATH, encoding='utf-8').read()
steps = []


def sub(pattern, repl, label, count=1):
    global src
    new, n = re.subn(pattern, repl, src, count=count, flags=re.S)
    steps.append((label, n))
    src = new


# resolve：带上列号
sub(r'    /// `Default` → 该列缺省值（未算出缺省值时原样返回）。\n'
    r'    #\[inline\]\n'
    r'    pub fn resolve_col\(&self, c: usize\) -> ColVal \{\n'
    r'        if c\.is_default\(\) && !self\.defaults\.is_empty\(\) \{\n'
    r'            self\.defaults\[c as usize\]\n'
    r'        \} else \{\n'
    r'            c\n'
    r'        \}\n'
    r'    \}',
    '''    /// `Default` → 该列缺省值（未算出缺省值时原样返回）。
    #[inline]
    pub fn resolve(&self, col: usize, v: ColVal) -> ColVal {
        if v.is_default() && !self.defaults.is_empty() {
            self.defaults[col]
        } else {
            v
        }
    }''', 'resolve')

# 调用点
sub(r'let changed = dead \|\| self\.resolve\(self\.rows\[row as usize\]\.get_col\(c\)\) != self\.resolve\(old\);',
    'let changed = dead\n'
    '                    || self.resolve(c, self.rows[row as usize].get_col(c)) != self.resolve(c, old);',
    'call-end_batch')
sub(r'if self\.resolve\(ra\.get_col\(c\)\) != self\.resolve\(rb\.get_col\(c\)\) \{',
    'if self.resolve(c, ra.get_col(c)) != self.resolve(c, rb.get_col(c)) {', 'call-diff')

# Diff 结构体
sub(r'(/// 一行的"被替换"记录)',
    '''/// 批间对比结果（列级）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Diff {
    pub added: u32,
    pub deleted: u32,
    pub changed: u32,
    pub changed_cols: u32,
}

\1''', 'Diff')

bad = [k for k, n in steps if n == 0]
io.open(PATH, 'w', encoding='utf-8', newline='\n').write(src)
print('steps:', steps)
if bad:
    print('!! 未生效：', bad, file=sys.stderr)
    sys.exit(1)
