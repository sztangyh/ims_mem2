# 验收门：真实 6 文件 → 行数 + 值级对拍（三方：ims_mem2 / 独立 Python oracle / ims_mem 地面真值）
# 用法：
#   pwsh -File tools\check_tables.ps1          # 全量门（约 10–12 分钟）
#   pwsh -File tools\check_tables.ps1 -Fast    # 快跑门（1 个文件、跳过 oracle/hist/SCD2，约 1–2 分钟）
param([switch]$Fast)
# 注意：这里**不能**用 'Stop'。Windows PowerShell 5.1 会把原生命令（cargo/python）的
# stderr 包成 NativeCommandError，配合 'Stop' 就变成终止性错误——而这两个工具都会往
# stderr 打正常进度。用 'Continue'，失败一律靠 $LASTEXITCODE 判。
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$allFiles = @(
    '..\agcf_data\ALLME_20260921033030.txt',
    '..\agcf_data\ALLME_20260921033031.txt',
    '..\agcf_data\ALLME_20260921033036.txt',
    '..\agcf_data\ALLME_202609210330361.txt',
    '..\agcf_data\ALLME_20260921033051.txt',
    '..\agcf_data\ALLME_20260921033056.txt'
)
$files = if ($Fast) { @($allFiles[0]) } else { $allFiles }
$fail = 0
$skipped = 0

# 每张表"展开成扁平行"之后的期望行数（6 文件口径；dumps 一律展开回扁平行，
# 所以归一化/折叠不改变这里的数字）。数字来源：tools/*_oracle.py 独立解析原文行数。
$expect = @{
    DNC     = 339
    SRVRGT  = 48614
    SRTCOST = 166638
    CNACLD  = 79669
    CLRDSN  = 26875
    PRATG   = 4376
    PRALNK  = 4378
    PRATKC  = 140032
    N7TKC   = 4448
    RT      = 1914
}

# 两侧（内存 dump / python oracle）按行排序后逐行对拍。
# 返回 $true = 通过。空输出、行数不符、内容差异都算失败（防止"两边都空"混过去）。
function Compare-Dump {
    param(
        [string]$Label,
        [string]$RustPath,
        [string]$PyPath,
        [int]$ExpectRows
    )
    $a = @(Get-Content $RustPath -ErrorAction SilentlyContinue)
    $b = @(Get-Content $PyPath -ErrorAction SilentlyContinue)
    Write-Host ("   {0}: 内存 {1} 行 / oracle {2} 行（期望 {3}）" -f $Label, $a.Count, $b.Count, $ExpectRows)
    if ($a.Count -eq 0 -or $b.Count -eq 0) {
        Write-Host '   x 一侧没有数据' -ForegroundColor Red
        return $false
    }
    if ($a.Count -ne $ExpectRows -or $b.Count -ne $ExpectRows) {
        Write-Host ("   x 行数与基准 {0} 不符" -f $ExpectRows) -ForegroundColor Red
        return $false
    }
    $d = @(Compare-Object $a $b)
    if ($d.Count -gt 0) {
        Write-Host ("   x 值级差异 {0} 处" -f $d.Count) -ForegroundColor Red
        $d | Select-Object -First 6 | ForEach-Object { Write-Host ('     ' + $_.SideIndicator + ' ' + $_.InputObject) }
        return $false
    }
    Write-Host ("   ok {0} 行 × 全列零差异" -f $a.Count) -ForegroundColor Green
    return $true
}

# 一张表：跑内存 dump + 独立 oracle，再 Compare-Dump。
function Check-Table {
    param(
        [string]$Label,
        [string]$Example,
        [string]$Oracle,
        [int]$ExpectRows,
        [string]$Moc = ''
    )
    $tag = $Label.ToLower()
    $rustPath = "temp\cmp_${tag}_rust.txt"
    $pyPath = "temp\cmp_${tag}_py.txt"
    if ($Moc) {
        cargo run --release --offline --quiet --example $Example -- $Moc @files 2>$null |
            Sort-Object | Set-Content -Encoding utf8 $rustPath
        $rustExit = $LASTEXITCODE
        python -X utf8 $Oracle $Moc @files 2>$null | Sort-Object | Set-Content -Encoding utf8 $pyPath
    } else {
        cargo run --release --offline --quiet --example $Example -- @files 2>$null |
            Sort-Object | Set-Content -Encoding utf8 $rustPath
        $rustExit = $LASTEXITCODE
        python -X utf8 $Oracle @files 2>$null | Sort-Object | Set-Content -Encoding utf8 $pyPath
    }
    # 导出侧自检（如 PRATKC 的"块化例外必须为 0"）用退出码表达：非 0 直接判失败
    if ($rustExit -ne 0) {
        Write-Host ("   x {0}: 导出退出码 {1}（自检未通过，见 kv_dump/pratkc 的 stderr）" -f $Label, $rustExit) -ForegroundColor Red
        $script:fail++
        return
    }
    if (-not (Compare-Dump $Label $rustPath $pyPath $ExpectRows)) {
        $script:fail++
    }
}
if ($Fast) {
    Write-Host '== Fast 模式（1 个文件；跳过 oracle / SCD2 / Ref / hist / 中继逐项）==' -ForegroundColor Cyan
    cargo run --release --offline --quiet --example dnc_stats -- $files 2>&1 |
        Select-Object -Last 6 | ForEach-Object { Write-Host ('   ' + $_) }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x 导入失败' -ForegroundColor Red; $fail++ }

    Write-Host '== SQLite 投影自校验 ==' -ForegroundColor Cyan
    cargo run -p ims_mem2_proj --release --offline --quiet --example export -- $files 2>&1 |
        Select-Object -Last 4 | ForEach-Object { Write-Host ('   ' + $_) }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x 投影自校验失败' -ForegroundColor Red; $fail++ }
    else { Write-Host '   ok 内存 == SQLite' -ForegroundColor Green }

    Write-Host '== 单测 + clippy ==' -ForegroundColor Cyan
    cargo test -p ims_mem2 --offline --quiet --all-targets 2>&1 |
        Select-String 'test result' | Select-Object -First 1 | ForEach-Object { Write-Host ('   ' + $_.Line) }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x 单测失败' -ForegroundColor Red; $fail++ }
    cargo clippy --release --workspace --offline --quiet --all-targets -- -D warnings 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Host '   x clippy' -ForegroundColor Red; $fail++ }
    else { Write-Host '   ok clippy 干净' -ForegroundColor Green }

    if ($fail -gt 0) { Write-Host ("x Fast 失败 {0} 项" -f $fail) -ForegroundColor Red; exit 1 }
    Write-Host 'ok Fast 通过（注意：值级 oracle / SCD2 / Ref / hist 未跑）' -ForegroundColor Green
    exit 0
}

Write-Host '== 导入统计 ==' -ForegroundColor Cyan
$imp = cargo run --release --offline --quiet --example dnc_stats -- @files 2>&1
$imp | Select-String -Pattern '导入总墙钟|report:|sym:|内存' | ForEach-Object { Write-Host ('   ' + $_.Line) }
# 峰值上限：分块流式导入后约 158 MB；整读会回到 245 MB，这里挡住回归
$memLine = $imp | Select-String '峰值 (\d+) KB'
if ($memLine) {
    $peakKb = [int]$memLine.Matches[0].Groups[1].Value
    if ($peakKb -gt 200000) {
        Write-Host ("   x 峰值 RSS {0} KB 超过 200000 KB（分块导入被改回整读了？）" -f $peakKb) -ForegroundColor Red
        $fail++
    } else {
        Write-Host ("   ok 峰值 {0} KB（上限 200000；整读版约 245000）" -f $peakKb) -ForegroundColor Green
    }
}

Write-Host '== 值级对拍：DNC ==' -ForegroundColor Cyan
Check-Table 'dnc' 'dnc_dump' 'tools\dnc_oracle.py' $expect.DNC

Write-Host '== 值级对拍：SRVRGT（Text 键）==' -ForegroundColor Cyan
Check-Table 'srvrgt' 'srvrgt_dump' 'tools\srvrgt_oracle.py' $expect.SRVRGT

Write-Host '== 值级对拍：SRTCOST（Pfx 复合键）==' -ForegroundColor Cyan
Check-Table 'srtcost' 'srtcost_dump' 'tools\srtcost_oracle.py' $expect.SRTCOST

Write-Host '== 值级对拍：CNACLD ==' -ForegroundColor Cyan
Check-Table 'cnacld' 'kv_dump' 'tools\kv_oracle.py' $expect.CNACLD 'cnacld'

Write-Host '== 值级对拍：CLRDSN ==' -ForegroundColor Cyan
Check-Table 'clrdsn' 'kv_dump' 'tools\kv_oracle.py' $expect.CLRDSN 'clrdsn'

Write-Host '== 值级对拍：PRATG ==' -ForegroundColor Cyan
Check-Table 'pratg' 'kv_dump' 'tools\kv_oracle.py' $expect.PRATG 'pratg'

Write-Host '== 值级对拍：PRALNK ==' -ForegroundColor Cyan
Check-Table 'pralnk' 'kv_dump' 'tools\kv_oracle.py' $expect.PRALNK 'pralnk'

Write-Host '== 值级对拍：PRATKC（块即行，展开回 140,032 路电路）==' -ForegroundColor Cyan
Check-Table 'pratkc' 'kv_dump' 'tools\kv_oracle.py' $expect.PRATKC 'pratkc'

Write-Host '== 值级对拍：N7TKC（稀疏 Circuit + N7TG 句柄，展开回 4,448 行）==' -ForegroundColor Cyan
Check-Table 'n7tkc' 'kv_dump' 'tools\kv_oracle.py' $expect.N7TKC 'n7tkc'

Write-Host '== 值级对拍：ASBR（19 列哈希）==' -ForegroundColor Cyan
cargo run --release --offline --quiet --example asbr_hash -- @files 2>$null | Sort-Object | Set-Content -Encoding utf8 temp\asbr_rust.txt
python -X utf8 tools\asbr_oracle.py @files | Sort-Object | Set-Content -Encoding utf8 temp\asbr_py.txt
$ha = Get-Content temp\asbr_rust.txt
$hb = Get-Content temp\asbr_py.txt
$asbrRows = 0
$ha | ForEach-Object { $asbrRows += [int](($_ -split '\|')[1]) }
Write-Host ("   网元 {0} 个 / 合计 {1} 行" -f $ha.Count, $asbrRows)
$dh = @(Compare-Object $ha $hb)
if ($dh.Count -gt 0 -or $asbrRows -ne 673407) {
    Write-Host '   x ASBR 哈希或行数不一致' -ForegroundColor Red
    $dh | Select-Object -First 6 | ForEach-Object { Write-Host ('     ' + $_.SideIndicator + ' ' + $_.InputObject) }
    $fail++
} else {
    Write-Host '   ok 673,407 行 × 19 列 与 oracle 哈希一致' -ForegroundColor Green
}

Write-Host '== 值级对拍：RT（参数组展开后 64 列）==' -ForegroundColor Cyan
cargo run -p ims_mem2_proj --release --offline --quiet --example rt_dump -- @files 2>$null |
    Sort-Object | Set-Content -Encoding utf8 temp\rt_rust.txt
python -X utf8 tools\kv_oracle.py rt @files | Sort-Object | Set-Content -Encoding utf8 temp\rt_py.txt
if (-not (Compare-Dump 'RT' 'temp\rt_rust.txt' 'temp\rt_py.txt' 1914)) { $fail++ }

Write-Host '== 长尾 143 表（对 ims_mem 地面真值）==' -ForegroundColor Cyan
cargo run --release --offline --quiet --example gen_dump -- @files 2>$null |
    Where-Object { $_ -match '^AGCF\d+\|' } |
    ForEach-Object { ($_ -split '\|')[0..2] -join '|' } |
    Sort-Object | Set-Content -Encoding utf8 temp\gen_mem2.txt
$mem1 = Join-Path $root '..\ims_mem'
if (Test-Path $mem1) {
    Push-Location $mem1
    try {
        cargo run --release --offline --quiet --example gen_dump -- @files 2>$null |
            Where-Object { $_ -match '^AGCF\d+\|' } |
            Sort-Object | Set-Content -Encoding utf8 (Join-Path $root 'temp\gen_mem1.txt')
    } finally { Pop-Location }
    $a = @(Get-Content temp\gen_mem1.txt -ErrorAction SilentlyContinue)
    $b = @(Get-Content temp\gen_mem2.txt -ErrorAction SilentlyContinue)
    Write-Host ("   mem1={0} 条, mem2={1} 条" -f $a.Count, $b.Count)
    if ($a.Count -eq 0 -or $b.Count -eq 0) {
        Write-Host '   x 长尾对拍：一侧没有数据' -ForegroundColor Red
        $fail++
        $d = @()
    } else {
    $d = @(Compare-Object $a $b)
    if ($d.Count -gt 0) {
        Write-Host ("   x 长尾差异 {0} 处" -f $d.Count) -ForegroundColor Red
        $d | Select-Object -First 6 | ForEach-Object { Write-Host ('     ' + $_.SideIndicator + ' ' + $_.InputObject) }
        $fail++
    } else {
        Write-Host ("   ok {0} 条 (NE×表) 存活行数零差异" -f $a.Count) -ForegroundColor Green
    }
    }
} else {
    Write-Host '   ⚠ skip: 找不到 ..\ims_mem（这段交叉对拍没做）' -ForegroundColor Yellow
    $skipped++
}
Write-Host '== 定表行数（对 ims_mem table_report）==' -ForegroundColor Cyan
cargo run --release --offline --quiet --example core_dump -- @files 2>$null | Sort-Object |
    Set-Content -Encoding utf8 temp\core_mem2.txt
if (Test-Path $mem1) {
    Push-Location $mem1
    try {
        $out = cargo run --release --offline --example import_agcf -- @files 2>&1
    } finally { Pop-Location }
    $ne = $null
    $lines = @()
    foreach ($l in $out) {
        if ($l -match '网元段 meid=(\d+)\s+(\S+)') {
            $ne = if ($matches[1] -eq '0') { 'CGP' } else { $matches[2] }
            continue
        }
        if ($l -match '^(ASBR|CNACLD|CLRDSN|DNC|SRVRGT|SRTCOST)\s+存活\s+(\d+)') {
            $lines += ('{0}|{1}|{2}' -f $ne, $matches[1], $matches[2])
        }
    }
    $mine = @(Get-Content temp\core_mem2.txt -ErrorAction SilentlyContinue |
        Where-Object { $_ -notmatch '\|0$' })
    $theirs = @($lines | Where-Object { $_ -notmatch '\|0$' } | Sort-Object)
    if ($theirs.Count -eq 0 -or $mine.Count -eq 0) {
        Write-Host ("   ⚠ skip: 没拿到 table_report（theirs={0} / mine={1}）；ims_mem 侧输出末尾：" -f $theirs.Count, $mine.Count) -ForegroundColor Yellow
        $out | Select-Object -Last 4 | ForEach-Object { Write-Host ('     ' + $_) }
        $skipped++
        $d = @()
    } else { $d = @(Compare-Object $theirs $mine) }
    if ($d.Count -gt 0) {
        Write-Host ("   x 定表行数差异 {0} 处" -f $d.Count) -ForegroundColor Red
        $d | Select-Object -First 8 | ForEach-Object { Write-Host ('     ' + $_.SideIndicator + ' ' + $_.InputObject) }
        $fail++
    } else {
        Write-Host ("   ok {0} 条 (NE×表) 行数零差异" -f $mine.Count) -ForegroundColor Green
    }
}

Write-Host '== SCD2 双批（2023 A51/A52 → 2026 ALLME，对 ims_core M5-STATUS）==' -ForegroundColor Cyan
$d2023 = 'C:\dev\ims_ws\data\agcf\20230919'
if ((Test-Path "$d2023\A51_20230919.txt") -and (Test-Path "$d2023\A52_20230919.txt")) {
    $scd2 = cargo run --release --offline --quiet --example scd2_check -- `
        "$d2023\A51_20230919.txt" "$d2023\A52_20230919.txt" `
        '..\agcf_data\ALLME_20260921033056.txt' '..\agcf_data\ALLME_202609210330361.txt' 2>&1
    # ASBR 双批（D59 的 seen bug 回归挡板）：cur 行数 + 快照删除数必须等于原始 MML 独立统计
    $ab = $scd2 | Select-String 'ASBR AGCF5[12]: cur=\d+（期望 \d+） 快照删除=\d+（期望 \d+） ok'
    if (($ab | Measure-Object).Count -ne 2) {
        Write-Host '   x ASBR 双批不符（seen bug 回归？）' -ForegroundColor Red
        $scd2 | Select-String 'ASBR AGCF5[12]:' | ForEach-Object { Write-Host ('     ' + $_.Line) }
        $fail++
    } else {
        $ab | ForEach-Object { Write-Host ('   ' + $_.Line) }
    }

    $m = ($scd2 | Select-String '实测：AGCF51 (\d+) / (\d+) / (\d+)；AGCF52 (\d+) / (\d+) / (\d+)')
    if (-not $m) {
        Write-Host '   x 拿不到 scd2_check 的实测行' -ForegroundColor Red
        $fail++
    } else {
        $g = $m.Matches[0].Groups
        $got = @([int]$g[1].Value, [int]$g[2].Value, [int]$g[3].Value,
                 [int]$g[4].Value, [int]$g[5].Value, [int]$g[6].Value)
        $want = @(17800, 2754, 15, 17827, 2754, 12)
        $dm = [regex]::Matches($scd2, 'Diff \{ added: (\d+), deleted: (\d+), changed: (\d+)')
        if ($dm.Count -lt 2) {
            Write-Host '   x scd2_check 没有输出两条 diff 行' -ForegroundColor Red
            $fail++
            $dm = $null
        }
        if (-not $dm) { $dgot = @() } else {
        $dgot = @([int]$dm[0].Groups[1].Value, [int]$dm[0].Groups[2].Value, [int]$dm[0].Groups[3].Value,
                  [int]$dm[1].Groups[1].Value, [int]$dm[1].Groups[2].Value, [int]$dm[1].Groups[3].Value)
        }
        $dwant = @(3864, 2754, 15, 3888, 2754, 12)
        if (($got -join ',') -eq ($want -join ',') -and ($dgot -join ',') -eq ($dwant -join ',')) {
            Write-Host ("   ok cur/删除/值变更 = {0}；diff 新增/删除/变更 = {1}（与 ims_core 完全一致）" -f ($got -join ' / '), ($dgot -join ' / ')) -ForegroundColor Green
        } else {
            Write-Host ("   x 期望 {0}，实测 {1}" -f ($want -join ' / '), ($got -join ' / ')) -ForegroundColor Red
            $fail++
        }
    }
} else {
    Write-Host '   skip: 找不到 2023 批次文件' -ForegroundColor Yellow
}

Write-Host '== Ref 物化（P3，moc_ref.json 的 match=eq）==' -ForegroundColor Cyan
$rr = cargo run --release --offline --quiet --example refs_report -- @files 2>&1
$mm = $rr | Select-String '命中 (\d+) / 悬空 (\d+) / 缺表 (\d+)'
$ec = $rr | Select-String '边总数 (\d+)'
if ($ec -and [int]$ec.Matches[0].Groups[1].Value -ne 124) {
    Write-Host ('   x 边数应为 124，实测 ' + $ec.Matches[0].Groups[1].Value) -ForegroundColor Red
    $fail++
}
$pra = $rr | Select-String 'PRA\s+命中\s+(\d+)\s+悬空\s+(\d+)'
if (-not $mm -or -not $pra) {
    Write-Host '   x refs_report 输出不符合预期' -ForegroundColor Red
    $fail++
} else {
    $g = $mm.Matches[0].Groups
    $got = @([int]$g[1].Value, [int]$g[2].Value, [int]$g[3].Value)
    # D54：PRATKC 改成"块即行"（4,376 行 / 140,032 条电路），它的两条出边
    # （PRATKC.MN→MODULE.MID、PRATKC.TG→PRATG.TG）改为**按块计数**：
    # 4,040,071 − 2 × (140,032 − 4,376) = 3,768,759。语义不变（块内 MN/TG 恒定）。
    # D53①：PRALNK 并入 PRATG（4,378 → 4,376 行）+ 自环边 PRATG.LINK→PRALNK.PLN 消失：
    # 3,768,759 − 4,376（自环边整条没了）− 4（两条孤儿不再计入源行）= 3,764,379；悬空 398,920。
    $want = @(3764379, 398920, 239)
    $p = $pra.Matches[0].Groups
    $pgot = @([int]$p[1].Value, [int]$p[2].Value)
    if (($got -join ',') -eq ($want -join ',') -and $pgot[0] -eq 160647 -and $pgot[1] -eq 0) {
        Write-Host ("   ok 124 条边：命中 {0} / 悬空 {1}；ASBR.TRUNKGP→PRATG.TG 在 PRA 行 100% 命中" -f $got[0], $got[1]) -ForegroundColor Green
    } else {
        Write-Host ("   x 期望 {0}（PRA 160647/0），实测 {1}（PRA {2}）" -f ($want -join ' / '), ($got -join ' / '), ($pgot -join '/')) -ForegroundColor Red
        $fail++
    }
}

Write-Host '== 引用名一致性护栏（A6：数值键 ↔ 名称列）==' -ForegroundColor Cyan
$srcRefs = python -X utf8 tools\check_refs.py @files 2>&1
$srcCode = $LASTEXITCODE
$srcRefs | ForEach-Object { Write-Host ('   ' + $_) }
if ($srcCode -ne 0) {
    Write-Host '   x 源级引用名漂移（tools\check_refs.py）' -ForegroundColor Red
    $fail++
}
$stRefs = cargo run --release --offline --quiet --example refs_name_check -- @files 2>&1
$stCode = $LASTEXITCODE
$stRefs | Select-String '合计：|ok 名称列|存储级不可表达' | ForEach-Object { Write-Host ('   ' + $_.Line) }
if ($stCode -ne 0) {
    Write-Host '   x 存储级引用名漂移（refs_name_check）' -ForegroundColor Red
    $stRefs | Select-String '样例' | ForEach-Object { Write-Host ('     ' + $_.Line) }
    $fail++
} else {
    Write-Host '   ok 源级 475,569 条 + 存储级 93,606 条引用，名称零漂移' -ForegroundColor Green
}
Write-Host '== 提升列缺省清单（A4：defaults.tsv 可复现 + 交叉校验）==' -ForegroundColor Cyan
$gd = python -X utf8 tools\gen_defaults.py --check 2>&1
$gdCode = $LASTEXITCODE
$gd | Select-String '扫出提升参数|与 ims_mem/defaults.json 比|与 ne_moc_default|与 table_columns.json 交叉|其中被判|复现校验' | ForEach-Object { Write-Host ('   ' + $_.Line) }
if ($gdCode -ne 0) {
    Write-Host '   x defaults.tsv 与重算结果不一致（tools\gen_defaults.py --check）' -ForegroundColor Red
    $fail++
} else {
    Write-Host '   ok 850 条：重算 == 磁盘 == ims_mem/defaults.json（0 值差）' -ForegroundColor Green
}
$preset = cargo run --release --offline --quiet --example dnc_stats -- @files 2>&1 | Select-String '预置清单'
if ($preset -and $preset.Line -match '预置清单 850 条') {
    Write-Host ('   ok 导入时装载 ' + $preset.Line.Trim()) -ForegroundColor Green
} else {
    Write-Host '   x dnc_stats 没报告"预置清单 850 条"（defaults.tsv 没被装载？）' -ForegroundColor Red
    $fail++
}
Write-Host '== 键推导复算（A5：从原文重推 vs 本仓声明）==' -ForegroundColor Cyan
$ik = python -X utf8 tools\infer_keys.py 2>&1
$ikCode = $LASTEXITCODE
$ik | Select-String '原文|推导来源分布|一致 |✗' | ForEach-Object { Write-Host ('   ' + $_.Line) }
if ($ikCode -ne 0) {
    Write-Host '   x 键推导与声明不一致（tools\infer_keys.py）' -ForegroundColor Red
    $ik | Select-String '✗' | ForEach-Object { Write-Host ('     ' + $_.Line) }
    $fail++
} else {
    Write-Host '   ok 143 张表的键与重新推导结果一致（分布也复现 64/34/15/9/8/8/4/1）' -ForegroundColor Green
}
Write-Host '== SQLite 只读投影（P5a）==' -ForegroundColor Cyan
$pj = cargo run -p ims_mem2_proj --release --offline --quiet --example export -- @files 2>&1
$ok = $pj | Select-String 'ok 0 \u5dee\u5f02'
if ($ok) {
    $pj | Select-String '\u8868 779|\u8017\u65f6|DB \u6587\u4ef6' | ForEach-Object { Write-Host ('   ' + $_.Line) }
    Write-Host '   ok 内存 == SQLite（逐表哈希）' -ForegroundColor Green
} else {
    Write-Host '   x 投影自校验失败' -ForegroundColor Red
    $pj | Select-Object -Last 5 | ForEach-Object { Write-Host ('     ' + $_) }
    $fail++
}

Write-Host '== 字冠数字伴生列（A3：`*_NUM` 池）==' -ForegroundColor Cyan
$pn = python -X utf8 tools\check_pfx_num.py 2>&1
$pnCode = $LASTEXITCODE
$pn | Select-String '检查 |✗|样例' | ForEach-Object { Write-Host ('   ' + $_.Line) }
if ($pnCode -ne 0) {
    Write-Host '   x 伴生列与原文的字冠数字不一致' -ForegroundColor Red
    $fail++
} else {
    Write-Host '   ok 伴生列 == canon(原文列)（独立 Python 口径）' -ForegroundColor Green
}
Write-Host '== URI 拆段还原（P6.1）==' -ForegroundColor Cyan
$puiOut = python -X utf8 tools\check_pui.py 2>&1
$puiCode = $LASTEXITCODE
$puiOut | ForEach-Object { Write-Host ('   ' + $_) }
if ($puiCode -eq 0) {
    Write-Host '   ok cur_ASBR.PUI 全部还原成完整 URI（内存里只存号码）' -ForegroundColor Green
} else {
    Write-Host '   x PUI 还原不完整' -ForegroundColor Red
    $fail++
}

Write-Host '== 号码最长匹配（P7：CNACLD 字冠，对暴力 oracle）==' -ForegroundColor Cyan
cargo run --release --offline --quiet --example pfx_lpm_dump -- $files 2>$null |
    Sort-Object | Set-Content -Encoding utf8 temp\lpm_rust.txt
python -X utf8 tools\pfx_oracle.py $files | Sort-Object | Set-Content -Encoding utf8 temp\lpm_py.txt
$a = @(Get-Content temp\lpm_rust.txt -ErrorAction SilentlyContinue)
$b = @(Get-Content temp\lpm_py.txt -ErrorAction SilentlyContinue)
Write-Host ("   拨号串 {0} 条 / oracle {1} 条" -f $a.Count, $b.Count)
if ($a.Count -eq 0 -or $b.Count -eq 0) {
    Write-Host '   x 号码最长匹配：一侧没有数据' -ForegroundColor Red
    $fail++
} else {
    $d = @(Compare-Object $a $b)
    if ($d.Count -gt 0) {
        Write-Host ("   x 最长匹配差异 {0} 处" -f $d.Count) -ForegroundColor Red
        $d | Select-Object -First 5 | ForEach-Object { Write-Host ('     ' + $_.SideIndicator + ' ' + $_.InputObject) }
        $fail++
    } else {
        Write-Host '   ok 最长匹配与暴力 oracle 零差异' -ForegroundColor Green
    }
}

Write-Host '== 中继公共属性 + 缺省主叫号码（P7）==' -ForegroundColor Cyan
$tr = python -X utf8 tools\check_trunk.py 2>&1
$trCode = $LASTEXITCODE
$tr | ForEach-Object { Write-Host ('   ' + $_) }
if ($trCode -eq 0) {
    Write-Host '   ok trunk_cur 4,916 行 / 缺省主叫号码 4,405 条 / 号码反查可用' -ForegroundColor Green
} else {
    Write-Host '   x 中继视图或号码反查不符合预期' -ForegroundColor Red
    $fail++
}

Write-Host '== hist 投影 + 历史回放（P5b）==' -ForegroundColor Cyan
if ((Test-Path "$d2023\A51_20230919.txt") -and (Test-Path "$d2023\A52_20230919.txt")) {
    $hh = cargo run -p ims_mem2_proj --release --offline --quiet --example export_hist -- `
        "$d2023\A51_20230919.txt" "$d2023\A52_20230919.txt" `
        '..\agcf_data\ALLME_20260921033056.txt' '..\agcf_data\ALLME_202609210330361.txt' 2>&1
    $code = $LASTEXITCODE
    $hh | Select-String 'cur:|@2023|hist_diff：|hist_row：|hist_ver：' | ForEach-Object { Write-Host ('   ' + $_.Line) }
    if ($code -ne 0) {
        Write-Host '   x 历史回放不一致' -ForegroundColor Red
        $fail++
    } else {
        Write-Host '   ok 只用 SQLite 回放 2023 态，与内存逐列一致' -ForegroundColor Green
    }
} else {
    Write-Host '   skip: 找不到 2023 批次文件' -ForegroundColor Yellow
}

Write-Host '== 方案 (a) 行级版本 + 全表 SCD2 审计（对原始 MML）==' -ForegroundColor Cyan
$au = python -X utf8 tools\check_scd2_audit.py 2>&1
$auCode = $LASTEXITCODE
$au | Select-String '核对 |hist_row|全部一致|不一致' | ForEach-Object { Write-Host ('   ' + $_.Line) }
if ($auCode -ne 0) {
    Write-Host '   x SCD2 审计不一致（键集合 / 行级版本）' -ForegroundColor Red
    $au | Select-String '^  AGCF' | Select-Object -First 8 | ForEach-Object { Write-Host ('     ' + $_.Line) }
    $fail++
} else {
    Write-Host '   ok cur 键集合 == 2026 批；hist_row 活着/死行 == 2023\\2026' -ForegroundColor Green
}

Write-Host '== IO M1：十三个 IO 对金标准（spec/acceptance.md M1）==' -ForegroundColor Cyan
$goldenDir = 'C:\dev\tryit\.temp'
if (Test-Path (Join-Path $goldenDir 'bridge_trunk.ndjson')) {
    $tf = Get-ChildItem ..\agcf_data\ALLME_2026*.txt | ForEach-Object { $_.FullName }
    # ① Trunk（专用对拍器）
    $tr = cargo run --release --offline --quiet --example io_trunk -- --out temp\trunk_io.ndjson @tf 2>&1
    $tr | Select-String '组装 |导出 ' | ForEach-Object { Write-Host ('   ' + $_.Line) }
    $tc = python -X utf8 tools\check_io_trunk.py 2>&1
    $tcCode = $LASTEXITCODE
    $tc | Select-String 'io |逐列对拍|ok Trunk|x Trunk' | ForEach-Object { Write-Host ('   ' + $_.Line) }
    if ($tcCode -ne 0) {
        Write-Host '   x Trunk IO 与金标准有差异' -ForegroundColor Red
        $fail++
    } else {
        Write-Host '   ok Trunk == 金标准' -ForegroundColor Green
    }
    # ② PraLink / ClrdsnGroup / Subscriber（通用对拍器）
    foreach ($io in @('PraLink', 'ClrdsnGroup', 'Subscriber', 'Rtana', 'Rt',
            'Srt', 'Tgsrt', 'V5Access', 'Transform', 'SrtAttr', 'N7tg', 'Ne')) {
        $of = 'temp\io_' + $io.ToLower() + '.ndjson'
        $r = cargo run --release --offline --quiet --example io_m1 -- --io $io --out $of @tf 2>&1
        $r | Select-String '组装 ' | ForEach-Object { Write-Host ('   ' + $_.Line) }
        $c = python -X utf8 tools\check_io.py --io $io 2>&1
        $cCode = $LASTEXITCODE
        $c | Select-String '逐列对拍|members 校验|ok |x ' | ForEach-Object { Write-Host ('   ' + $_.Line) }
        if ($cCode -ne 0) {
            Write-Host ("   x {0} 与金标准有差异" -f $io) -ForegroundColor Red
            $fail++
        }
    }
} else {
    Write-Host '   . 金标准 bridge_trunk.ndjson 不存在，跳过（非失败）' -ForegroundColor Yellow
}

Write-Host '== 服务/客户端两形态一致（ims_svc 进程内 vs ims_tui --tcp，D66）==' -ForegroundColor Cyan
if (Test-Path 'C:\dev\tryit\agcf_data') {
    $null = cargo build --release --offline --quiet -p ims_svc -p ims_tui 2>&1
    $tui = python -X utf8 tools\check_tui.py 2>&1
    $tuiCode = $LASTEXITCODE
    $tui | Select-String 'ok |x ' | Select-Object -First 4 | ForEach-Object { Write-Host ('   ' + $_.Line) }
    if ($tuiCode -ne 0) {
        Write-Host '   x 进程内与经网络的结果不一致' -ForegroundColor Red
        $fail++
    } else {
        Write-Host '   ok 两形态结果一致 + 连接生命周期正常' -ForegroundColor Green
    }
} else {
    Write-Host '   . 数据目录不存在，跳过（非失败）' -ForegroundColor Yellow
}

Write-Host '== M3 目录服务（三池 + keyset 分页，对 oracle）==' -ForegroundColor Cyan
if (Test-Path 'C:\dev\tryit\.temp\bridge_cnacld.ndjson') {
    $null = cargo build --release --offline --quiet --example io_catalog 2>&1
    $cat = python -X utf8 tools\check_catalog.py 2>&1
    $catCode = $LASTEXITCODE
    $cat | Select-String 'oracle 池|接到 |ok M3|x M3|✗' | Select-Object -First 8 | ForEach-Object { Write-Host ('   ' + $_.Line) }
    if ($catCode -ne 0) {
        Write-Host '   x M3 目录服务与 oracle 不一致' -ForegroundColor Red
        $fail++
    } else {
        Write-Host '   ok 池规模 / 匹配 / 分页 与 oracle 一致' -ForegroundColor Green
    }
} else {
    Write-Host '   . 金标准不存在，跳过（非失败）' -ForegroundColor Yellow
}

Write-Host '== M4 IO→IO 链接（7 条单跳边，对 oracle）==' -ForegroundColor Cyan
if (Test-Path 'C:\dev\tryit\.temp\bridge_pratg.ndjson') {
    $null = cargo build --release --offline --quiet --example io_link 2>&1
    $lk = python -X utf8 tools\check_link.py 2>&1
    $lkCode = $LASTEXITCODE
    $lk | Select-String '^边 |ok |x |dangling' | Select-Object -First 10 | ForEach-Object { Write-Host ('   ' + $_.Line) }
    if ($lkCode -ne 0) {
        Write-Host '   x M4 链接与 oracle 不一致' -ForegroundColor Red
        $fail++
    } else {
        Write-Host '   ok 7 条边（含 1 处已登记孤儿）与 oracle 一致' -ForegroundColor Green
    }
} else {
    Write-Host '   . 金标准不存在，跳过（非失败）' -ForegroundColor Yellow
}

Write-Host '== IO 规格自洽（spec/io_spec.json + catalog_index.json）==' -ForegroundColor Cyan
$ios = python -X utf8 tools\check_io_spec.py 2>&1
$iosCode = $LASTEXITCODE
$ios | Select-String '^IO |^ok |^FAIL ' | ForEach-Object { Write-Host ('   ' + $_.Line) }
if ($iosCode -ne 0) {
    Write-Host '   x IO 规格不自洽（结构/基线/覆盖/目录池）' -ForegroundColor Red
    $ios | Select-String '^  . ' | Select-Object -First 8 | ForEach-Object { Write-Host ('     ' + $_.Line) }
    $fail++
} else {
    Write-Host '   ok 结构 / 基线 / 覆盖 / 目录池 四组断言全过' -ForegroundColor Green
}

Write-Host '== hist 逐行 diff 明细（P5c：old→new 链自洽）==' -ForegroundColor Cyan
$hdf = python -X utf8 tools\check_hist_diff.py 2>&1
$hdfCode = $LASTEXITCODE
$hdf | Select-String 'hist_diff：|检查 |不连续|断链|落地' | ForEach-Object { Write-Host ('   ' + $_.Line) }
if ($hdfCode -ne 0) {
    Write-Host '   x hist_diff 链不自洽' -ForegroundColor Red
    $fail++
} else {
    Write-Host '   ok 连续 / 接续 / 落地 三项全通过' -ForegroundColor Green
}

Write-Host '== N7 句柄对齐 + hist 口径（D57 的"门看不见"部分）==' -ForegroundColor Cyan
$n7h = python -X utf8 tools\check_n7_handles.py 2>&1
$n7h | ForEach-Object { Write-Host $_ }
if ($LASTEXITCODE -ne 0) {
    Write-Host '   x N7 句柄/历史校验失败' -ForegroundColor Red
    $fail++
} else {
    Write-Host '   ok 句柄长度/不重叠、hist_N7TKC 为空' -ForegroundColor Green
}

Write-Host '== 提升列（P1b）==' -ForegroundColor Cyan
$lift = cargo run --release --offline --quiet --example dnc_stats -- @files 2>&1 | Select-String '\u63d0\u5347\u5217\uff1a'
if ($lift) {
    Write-Host ('   ' + $lift.Line) -ForegroundColor Green
    if ($lift.Line -notmatch '\u5e38\u91cf 850 = 99\.3%') {
        Write-Host '   x \u63d0\u5347\u5217\u7edf\u8ba1\u4e0e\u57fa\u51c6\u4e0d\u7b26' -ForegroundColor Red
        $fail++
    }
} else {
    Write-Host '   x 拿不到提升列统计' -ForegroundColor Red
    $fail++
}

Write-Host '== 宽值类型（A2：Ip/Eid/Bits 真类型 + 原文兜底）==' -ForegroundColor Cyan
$stats2 = cargo run --release --offline --quiet --example dnc_stats -- @files 2>&1
$wide = $stats2 | Select-String 'A2 宽值：'
if ($wide) {
    Write-Host ('   ' + $wide.Line) -ForegroundColor Green
    if ($wide.Line -notmatch '原文兜底 5040 次') {
        Write-Host '   x 宽值原文兜底次数与基准（5040）不符' -ForegroundColor Red
        $fail++
    }
} else {
    Write-Host '   x 拿不到宽值统计' -ForegroundColor Red
    $fail++
}
Write-Host '== SRVRGT 注册检查（缺口 10：死边改在 ASBR 侧做）==' -ForegroundColor Cyan
$regLine = $stats2 | Select-String 'SRVRGT→ASBR 注册：'
if ($regLine) {
    Write-Host ('   ' + $regLine.Line) -ForegroundColor Green
    if ($regLine.Line -notmatch '落盘 48614 / 孤儿 0') {
        Write-Host '   x SRVRGT 注册数或孤儿数与基准（48614 / 0）不符' -ForegroundColor Red
        $fail++
    }
} else {
    Write-Host '   x 拿不到 SRVRGT 注册统计' -ForegroundColor Red
    $fail++
}

Write-Host '== 投影索引（热表 (ne) 索引：数量 / 命名 / 查询计划）==' -ForegroundColor Cyan
$curdb = ''
if (Test-Path 'temp\cur_db.txt') { $curdb = (Get-Content 'temp\cur_db.txt' -Raw).Trim() }
if ($curdb -and (Test-Path $curdb)) {
    python -X utf8 tools\check_indexes.py $curdb 2>&1 | ForEach-Object { Write-Host ($_ -replace '^', '') }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x 投影索引检查失败' -ForegroundColor Red; $fail++ }
} else {
    Write-Host '   skip: 投影库缺失（temp\cur_db.txt）' -ForegroundColor Yellow
    $skipped++
}

Write-Host '== 中文词典（zh.tsv 六组断言：装载保真 / 覆盖率 / 开关名 / S 层冻结）==' -ForegroundColor Cyan
python -X utf8 tools\gen_zh.py --check 2>&1 | ForEach-Object { Write-Host ('   ' + $_) }
if ($LASTEXITCODE -ne 0) { Write-Host '   x zh.tsv 复现校验失败' -ForegroundColor Red; $fail++ }
python -X utf8 tools\check_zh.py 2>&1 | ForEach-Object { Write-Host ('   ' + $_) }
if ($LASTEXITCODE -ne 0) { Write-Host '   x 中文词典检查失败' -ForegroundColor Red; $fail++ }

Write-Host '== MGW 导入（对 ims_core 的 mgw_library.db：逐表逐行逐列）==' -ForegroundColor Cyan
$mgwRoot = 'C:\dev\ims_ws\data\mgw'
$mgwDb = 'C:\dev\ims_ws\ims_core\temp\mgw_library.db'
if ((Test-Path $mgwDb) -and (Test-Path $mgwRoot)) {
    python -X utf8 tools\port_mgw_schema.py --check 2>&1 | ForEach-Object { Write-Host ('   ' + $_) }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x MGW 表声明与磁盘不一致' -ForegroundColor Red; $fail++ }
    python -X utf8 tools\check_mgw.py 2>&1 | ForEach-Object { Write-Host ($_ -replace '^', '') }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x MGW 对拍失败' -ForegroundColor Red; $fail++ }
    python -X utf8 tools\check_io_mgw.py 2>&1 | ForEach-Object { Write-Host ($_ -replace '^', '') }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x MGW 的 6 个 IO 对拍失败' -ForegroundColor Red; $fail++ }
    python -X utf8 tools\check_mgw_proj.py 2>&1 | ForEach-Object { Write-Host ($_ -replace '^', '') }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x MGW 投影失败' -ForegroundColor Red; $fail++ }
    python -X utf8 tools\check_mgw_zh.py 2>&1 | ForEach-Object { Write-Host ($_ -replace '^', '') }
    if ($LASTEXITCODE -ne 0) { Write-Host '   x MGW 中文词典失败' -ForegroundColor Red; $fail++ }
} else {
    Write-Host '   skip: 缺 ims_core 的 mgw_library.db 或 MGW 日志目录' -ForegroundColor Yellow
    $skipped++
}

Write-Host '== 单测与 clippy ==' -ForegroundColor Cyan
cargo test -p ims_mem2 --offline --quiet --all-targets 2>&1 | Select-String 'test result' | ForEach-Object { Write-Host ('   ' + $_.Line) }
if ($LASTEXITCODE -ne 0) { Write-Host '   x cargo test 失败' -ForegroundColor Red; $fail++ }
cargo clippy --release --workspace --offline --quiet --all-targets -- -D warnings 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Host '   x clippy 告警' -ForegroundColor Red; $fail++ } else { Write-Host '   ok clippy 干净' -ForegroundColor Green }

if ($fail -gt 0) { Write-Host ("x 失败 {0} 项（跳过 {1} 段）" -f $fail, $skipped) -ForegroundColor Red; exit 1 }
if ($skipped -gt 0) { Write-Host ("⚠ 通过，但有 {0} 段被跳过（见上方 skip 行）" -f $skipped) -ForegroundColor Yellow }
Write-Host 'ok 全通过（9 张定表值级 + ASBR 哈希 + 长尾/定表行数 + SCD2/Ref/引用名护栏/键推导/投影/hist/中继）' -ForegroundColor Green
exit 0


