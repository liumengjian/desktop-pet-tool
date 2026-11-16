# 人脸比对模型分包：把 det_10g / w600k_r50 与 onnxruntime 依赖打成 COS 直传包，
# 并生成应用端（src-tauri/src/face/models.rs）读取的 manifest.json。
#
# 不写死 E:\ 盘符；默认取仓库内的 .preview 调试目录，可用参数覆盖。

param(
    [string]$ModelsDir = "",
    [string]$DepsDir = "",
    [string]$Out = "",
    [switch]$KeepStaging
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $ModelsDir) { $ModelsDir = Join-Path $repo '.preview\face-models\unzipped' }
if (-not $DepsDir)   { $DepsDir   = Join-Path $repo '.preview\face-deps' }
if (-not $Out)       { $Out       = Join-Path $repo '.preview\face-models\pack' }

$ModelsDir = (Resolve-Path -LiteralPath $ModelsDir).Path
$DepsDir   = (Resolve-Path -LiteralPath $DepsDir).Path
if (-not (Test-Path -LiteralPath $Out)) { New-Item -ItemType Directory -Path $Out -Force | Out-Null }
$Out = (Resolve-Path -LiteralPath $Out).Path

$det = Join-Path $ModelsDir 'det_10g.onnx'
$rec = Join-Path $ModelsDir 'w600k_r50.onnx'
foreach ($p in @($det, $rec)) {
    if (-not (Test-Path -LiteralPath $p)) { throw "缺少模型文件: $p" }
}
if (-not (Test-Path -LiteralPath (Join-Path $DepsDir 'onnxruntime\__init__.py'))) {
    throw "依赖目录不像 onnxruntime 安装目录（缺 onnxruntime\__init__.py）: $DepsDir"
}

# ---- 1. 组装依赖暂存目录：剔除 numpy（复用 pdfconv 已经装好的那份）----
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$staging = Join-Path ([System.IO.Path]::GetDirectoryName($Out)) "deps-stage-$stamp"
New-Item -ItemType Directory -Path $staging -Force | Out-Null

$excludeNames = @('numpy', 'numpy.libs', 'bin', '__pycache__')
Get-ChildItem -LiteralPath $DepsDir -Force | ForEach-Object {
    if ($excludeNames -contains $_.Name) { return }
    if ($_.Name -like 'numpy-*.dist-info') { return }
    if ($_.Name -like '*.pyc') { return }
    Copy-Item -LiteralPath $_.FullName -Destination $staging -Recurse -Force
}

# 深度清理字节码，减小包体
Get-ChildItem -LiteralPath $staging -Recurse -Force -Directory -Filter '__pycache__' |
    ForEach-Object {
        $resolved = $_.FullName
        if ($resolved.StartsWith($staging)) { Remove-Item -LiteralPath $resolved -Recurse -Force }
    }

if (-not (Test-Path -LiteralPath (Join-Path $staging 'onnxruntime\__init__.py'))) {
    throw "暂存目录缺少 onnxruntime\__init__.py，打包会失败（zip 必须平铺）"
}

# ---- 2. 打 zip：包内平铺，解压即落成 deps/onnxruntime 等 ----
$zip = Join-Path $Out 'face-deps-v1.zip'
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $staging, $zip,
    [System.IO.Compression.CompressionLevel]::Optimal,
    $false
) | Out-Null

# 校验 zip 根条目
$entries = [System.IO.Compression.ZipFile]::OpenRead($zip).Entries
$hasRuntime = $false
foreach ($e in $entries) {
    if ($e.FullName -eq 'onnxruntime/__init__.py') { $hasRuntime = $true; break }
}
if (-not $hasRuntime) { throw "zip 结构不对：根下没有 onnxruntime/__init__.py" }

# ---- 3. 生成 manifest.json（UTF-8 无 BOM，应用用 serde_json 直接解析）----
function Get-Sha([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLower()
}
function Get-Size([string]$Path) {
    (Get-Item -LiteralPath $Path).Length
}

$manifest = [ordered]@{
    models = @(
        [ordered]@{ name = 'det_10g.onnx';   sha256 = (Get-Sha $det); bytes = (Get-Size $det) },
        [ordered]@{ name = 'w600k_r50.onnx'; sha256 = (Get-Sha $rec); bytes = (Get-Size $rec) }
    )
    deps   = [ordered]@{ name = 'face-deps-v1.zip'; sha256 = (Get-Sha $zip); bytes = (Get-Size $zip) }
}

# 模型文件也复制到输出目录，上传时只看一个文件夹
Copy-Item -LiteralPath $det -Destination $Out -Force
Copy-Item -LiteralPath $rec -Destination $Out -Force

$json = $manifest | ConvertTo-Json -Depth 4
$manifestPath = Join-Path $Out 'manifest.json'
[IO.File]::WriteAllText($manifestPath, $json, (New-Object Text.UTF8Encoding($false)))

if (-not $KeepStaging) {
    if ($staging.StartsWith((Split-Path -Parent $Out))) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
}

# ---- 4. 汇报体积与上传清单 ----
$total = 0
foreach ($f in @('det_10g.onnx', 'w600k_r50.onnx', 'face-deps-v1.zip', 'manifest.json')) {
    $p = Join-Path $Out $f
    $mb = '{0:N1}' -f ((Get-Item -LiteralPath $p).Length / 1MB)
    $total += (Get-Item -LiteralPath $p).Length
    "  {0,-20} {1,8} MB" -f $f, $mb
}
"输出目录: $Out"
"合计: {0:N1} MB（上传时约 200MB，COS 存储按此估算）" -f ($total / 1MB)
""
"上传示例（ossutil）："
"  ossutil cp `"$Out\`" oss://prina-1413661498/tools/desktop-pet-tool/face-models/pack/ -r -f"
"最后再传 manifest.json，避免用户先拿到清单却下不到文件。"
