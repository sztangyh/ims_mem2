# -*- coding: utf-8 -*-
"""把 Table 的版本登记从"逐次 stage"改成"批初快照 + 批末差分"（D33）。

用法：python tools/patch_table_snapshot.py
（一次性补丁脚本；跑完可用 diff 审阅，之后可删）
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


# 1) 字段：pending → touched
sub(r'    /// 本批尚未落盘的变更[^\n]*\n    pending: HashMap<u32, Vec<\(u16, ColVal\)>>,',
    '    /// 本批第一次被改动的行 → **批初整行快照**；批末用它算差量（D33）\n'
    '    touched: HashMap<u32, H::Row>,', 'field')
sub(r'            pending: HashMap::new\(\),', '            touched: HashMap::new(),', 'field-init')

# 2) end_batch：快照删除 + 批末差分
new_end = '''    pub fn end_batch(&mut self) {
        // ① 快照删除：本批没出现过的存活行 = 被删除（先进 touched 拿批初快照）
        if self.snapshot {
            let mut dead: Vec<u32> = Vec::new();
            for i in self.alive.iter_ones() {
                if !self.seen.get(i) {
                    dead.push(i as u32);
                }
            }
            for i in dead {
                self.touch(i);
                self.alive.clear(i as usize);
                self.dead_at.insert(i, self.batch);
                self.stats.snapshot_deletes += 1;
            }
        }

        if self.touched.is_empty() {
            return;
        }
        let batch = self.batch;
        let mut touched = std::mem::take(&mut self.touched);
        let mut keys: Vec<u32> = touched.keys().copied().collect();
        keys.sort_unstable();
        for row in keys {
            let snap = touched.remove(&row).expect("刚取自 touched");
            let from = self.cur_from[row as usize];
            if from >= batch {
                continue; // 本批新建：没有更早的状态
            }
            let dead = self.dead_at.get(&row) == Some(&batch);
            let n = <H::Row as ColStore>::COLS;
            let mut cols = Vec::new();
            let mut vals = Vec::new();
            for c in 0..n {
                let old = snap.get_col(c);
                let changed = dead || self.rows[row as usize].get_col(c) != old;
                if changed {
                    cols.push(c);
                    vals.push(old);
                }
            }
            if cols.is_empty() {
                continue; // 值没变 → 不写版本（重放幂等）
            }
            self.vhist.record(row, from, batch, cols, vals);
            self.cur_from[row as usize] = batch;
            self.stats.versions += 1;
        }
    }

'''
sub(r'    pub fn end_batch\(&mut self\) \{.*?\n    \}\n\n', new_end, 'end_batch')

# 3) stage → touch
new_touch = '''    /// 首次改动某行时，留一份批初整行快照（批末据此算差量）。
    fn touch(&mut self, row: u32) {
        if self.cur_from[row as usize] >= self.batch {
            return; // 本批新建，无更早状态
        }
        if !self.touched.contains_key(&row) {
            let r = self.rows[row as usize].clone();
            self.touched.insert(row, r);
        }
    }

'''
sub(r'    /// 把一列的"旧值"记进本批 pending.*?\n    \}\n\n', new_touch, 'stage→touch')

# 4) merge_at
new_merge = '''    fn merge_at(&mut self, i: u32, p: &H::Pending, it: &Interner) {
        // 本批见过（MOD/SET 命中也算"本批出现"——否则快照删除会把它误杀）
        self.seen.set(i as usize);
        self.touch(i);
        if H::merge(&mut self.rows[i as usize], p, it).is_empty() {
            self.stats.unchanged += 1;
        }
    }

'''
sub(r'    fn merge_at\(&mut self, i: u32, p: &H::Pending, it: &Interner\) \{.*?\n    \}\n\n',
    new_merge, 'merge_at')

# 5) 删掉 record_full
sub(r'    /// 整行快照进 p[^\n]*\n    fn record_full\(&mut self, i: u32\) \{.*?\n    \}\n\n', '', 'record_full')

# 6) ADD 命中：touch
sub(r'                                self\.record_full\(i\);\n',
    '                                self.touch(i);\n', 'add-hit')

# 7) RMV：touch + dead_at
sub(r'                        self\.alive\.clear\(i as usize\);\n'
    r'                        if let Some\(k\) = &key \{\n'
    r'                            self\.index\.remove\(k\);\n'
    r'                        \}\n',
    '                        self.touch(i);\n'
    '                        self.alive.clear(i as usize);\n'
    '                        self.dead_at.insert(i, self.batch);\n'
    '                        if let Some(k) = &key {\n'
    '                            self.index.remove(k);\n'
    '                        }\n', 'rmv')

bad = [k for k, n in steps if n == 0]
io.open(PATH, 'w', encoding='utf-8', newline='\n').write(src)
print('steps:', steps)
if bad:
    print('!! 未生效：', bad, file=sys.stderr)
    sys.exit(1)
