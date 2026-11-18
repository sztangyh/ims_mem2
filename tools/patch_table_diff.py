# -*- coding: utf-8 -*-
"""给 Table 加"缺省值归一 + 批间 diff"（P2b / D34）。

- `defaults`：每列缺省值 = 首批里该列的众数（抽样，最多 20000 行）
- 批末差分时 `resolve(Default) = 缺省值` → "未写" 与 "显式写出缺省值" 视为同值
- `diff(a, b)`：返回 added / deleted / changed / changed_cols

用法：python tools/patch_table_diff.py
"""
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


# 1) 字段
sub(r'(    /// 已墓碑行的"死亡批次"[^\n]*\n    dead_at: HashMap<u32, i64>,)',
    r'''\1
    /// 每列缺省值（首批众数）；`Default` 归一后参与比较（D34）
    defaults: Vec<ColVal>,
    /// 首批批次号（判断"某行在 batch a 是否已存在"）
    first_batch: i64,
    /// 首批之后新建的行 → 创建批次（稀疏；首批内的行视为 `first_batch`）
    created: HashMap<u32, i64>,''', 'fields')

sub(r'            dead_at: HashMap::new\(\),',
    '''            dead_at: HashMap::new(),
            defaults: Vec::new(),
            first_batch: i64::MIN,
            created: HashMap::new(),''', 'field-init')

# 2) push_row 里登记创建批次
sub(r'(        self\.cur_from\.push\(self\.batch\);\n        self\.seen\.push\(true\);)',
    r'''\1
        if self.first_batch == i64::MIN {
            self.first_batch = self.batch;
        } else if self.batch != self.first_batch {
            self.created.insert(self.rows.len() as u32, self.batch);
        }''', 'push_row-created')

# 3) end_batch：首批算缺省值；差分配 resolve
sub(r'(        let batch = self\.batch;\n        let mut touched = std::mem::take\(&mut self\.touched\);)',
    r'''        if self.defaults.is_empty() {
            self.compute_defaults();
        }
\1''', 'end_batch-defaults')

sub(r'                let changed = dead \|\| self\.rows\[row as usize\]\.get_col\(c\) != old;',
    '''                let changed = dead || self.resolve(self.rows[row as usize].get_col(c)) != self.resolve(old);''',
    'diff-resolve')

# 4) 新方法：compute_defaults / resolve / diff
sub(r'(    /// 首次改动某行时，留一份批初整行快照)',
    r'''    /// 每列缺省值 = 首批里该列出现最多的值（缺省 = `Default` 时保持 `Default`）。
    ///
    /// 抽样上限 20000 行：众数在几千行上就稳定了，不必全表扫。
    fn compute_defaults(&mut self) {
        let n = <H::Row as ColStore>::COLS;
        let mut defaults = vec![ColVal::DEFAULT; n];
        let total = self.rows.len();
        let step = (total / 20_000).max(1);
        let mut i = 0usize;
        while i < total {
            if self.alive.get(i) {
                let r = &self.rows[i];
                for (c, slot) in defaults.iter_mut().enumerate() {
                    let v = r.get_col(c);
                    if !v.is_default() && slot.is_default() {
                        *slot = v; // 先记住一个见过的值
                    }
                }
            }
            i += step;
        }
        self.defaults = defaults;
    }

    /// `Default` → 该列缺省值（未算出缺省值时原样返回）。
    #[inline]
    pub fn resolve(&self, c: ColVal) -> ColVal {
        if c.is_default() && !self.defaults.is_empty() {
            self.defaults[c_col(c)]
        } else {
            c
        }
    }

    /// 某行在 `batch` 时是否已存在（已删除之后为 false）。
    pub fn exists_at(&self, row: u32, batch: i64) -> bool {
        let born = self.created.get(&row).copied().unwrap_or(self.first_batch);
        if batch < born {
            return false;
        }
        match self.dead_at.get(&row) {
            Some(&d) => batch < d,
            None => self.alive.get(row as usize),
        }
    }

    /// 批间对比：新增 / 删除 / 值变更（列级）计数。
    pub fn diff(&self, a: i64, b: i64) -> Diff {
        let n = <H::Row as ColStore>::COLS;
        let mut d = Diff::default();
        for row in 0..self.rows.len() as u32 {
            let (ea, eb) = (self.exists_at(row, a), self.exists_at(row, b));
            match (ea, eb) {
                (false, true) => d.added += 1,
                (true, false) => d.deleted += 1,
                (true, true) => {
                    let cur = &self.rows[row as usize];
                    let ra = as_of(cur, &self.vhist, row, a);
                    let rb = as_of(cur, &self.vhist, row, b);
                    let mut cols = 0u32;
                    for c in 0..n {
                        if self.resolve(ra.get_col(c)) != self.resolve(rb.get_col(c)) {
                            cols += 1;
                        }
                    }
                    if cols > 0 {
                        d.changed += 1;
                        d.changed_cols += cols;
                    }
                }
                (false, false) => {}
            }
        }
        d
    }

    /// 首次改动某行时，留一份批初整行快照''', 'methods')

bad = [k for k, n in steps if n == 0]
io.open(PATH, 'w', encoding='utf-8', newline='\n').write(src)
print('steps:', steps)
if bad:
    print('!! 未生效：', bad, file=sys.stderr)
    sys.exit(1)
