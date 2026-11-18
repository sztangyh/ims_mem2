# -*- coding: utf-8 -*-
"""把 `schema_gen_data.rs` 里的连续序号参数折成参数组（P4 / D8）。

判定（五条判据的机械化版本）：
1. 同一前缀、从 1 开始**连续**到 k；
2. k ≥ 5（太少不值得折）；
3. 组内类型**完全一致**（`perc29..32` 是 Text，就会把 `perc` 只折到 28）；
4. 位置语义（序号即下标）；
5. 不被单独查询（这里近似为"不是键列"）。

用法：python tools/fold_groups.py [--dry]
可重复执行（已经折过的块不会再折）。
"""
import io
import re
import sys

PATH = 'crates/core/src/schema_gen_data.rs'
BLOCK = re.compile(r'(mml_table!\s*\{\s*\n\s*([A-Z0-9_]+),\s*([A-Za-z0-9_]+),\s*\n(.*?)\n\s*\})', re.S)
FIELD = re.compile(r'^(\s*)([a-zA-Z_][a-zA-Z0-9_]*?)(\d+)\s*:\s*([A-Za-z][A-Za-z0-9_]*)\s*,\s*$')


def fold_block(body: str):
    """返回 (新 body, 折出的组清单)。"""
    lines = body.split('\n')
    # 找出所有 序号字段：prefix -> {n: (line_idx, ty, indent)}
    runs = {}
    for i, line in enumerate(lines):
        m = FIELD.match(line)
        if not m:
            continue
        indent, prefix, num, ty = m.group(1), m.group(2), int(m.group(3)), m.group(4)
        runs.setdefault((prefix, ty), {})[num] = (i, indent)
    # 已有同名裸字段（如 RT 的 `sr: U16`）就跳过该前缀，避免字段重名
    bare = set()
    for line in lines:
        mb = re.match(r'^\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*([A-Za-z][A-Za-z0-9_]*)\s*,\s*$', line)
        if mb:
            bare.add(mb.group(1))
    folded = {}
    for (prefix, ty), nums in runs.items():
        if prefix in bare:
            continue
        if ty not in ('U8', 'U16'):
            continue  # 宏目前只支持 U8[n]/U16[n] 组；Bool/Text 组留待后续
        k = 0
        while (k + 1) in nums:
            k += 1
        if k < 5 or sorted(nums) != list(range(1, k + 1)):
            # 只折"1..k 连续"的整段；后面多出来的（如 perc29..32 Text）自然分开
            if k < 5:
                continue
        idxs = [nums[n][0] for n in range(1, k + 1)]
        folded[prefix] = (ty, k, idxs, nums[1][1])
    if not folded:
        return body, []

    drop = set()
    for prefix, (ty, k, idxs, indent) in folded.items():
        drop.update(idxs)
    out = []
    info = []
    for i, line in enumerate(lines):
        if i in drop:
            # 在 x1 那一行位置写组声明
            for prefix, (ty, k, idxs, indent) in folded.items():
                if i == idxs[0]:
                    out.append(f'{indent}{prefix}: {ty}[{k}],')
                    info.append((prefix, ty, k))
            continue
        out.append(line)
    return '\n'.join(out), info


def main():
    dry = '--dry' in sys.argv
    src = io.open(PATH, encoding='utf-8').read()
    total = []

    def repl(m):
        whole, moc, ty, body = m.group(1), m.group(2), m.group(3), m.group(4)
        new_body, info = fold_block(body)
        for (prefix, fty, k) in info:
            total.append((moc, prefix, fty, k))
        if not info:
            return whole
        return f'mml_table! {{\n    {moc}, {ty},\n{new_body}\n}}'

    out = BLOCK.sub(repl, src)
    print(f'折出 {len(total)} 个参数组：')
    for moc, prefix, fty, k in total[:20]:
        print(f'  {moc:12} {prefix}1..{k} ({fty})')
    if len(total) > 20:
        print(f'  … 共 {len(total)} 个')
    if not dry:
        io.open(PATH, 'w', encoding='utf-8', newline='\n').write(out)
        print('已写回', PATH)


if __name__ == '__main__':
    main()
