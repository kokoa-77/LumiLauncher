#Requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $TargetDirectory,

    [Parameter(Mandatory)]
    [string] $OutputDirectory,

    [string] $Version,

    [switch] $Unsigned
)

$ErrorActionPreference = 'Stop'

if (!$Version) {
    $tauriConfig = Join-Path $PSScriptRoot '../../app/src-tauri/tauri.conf.json'
    $Version = (Get-Content -LiteralPath $tauriConfig -Raw | ConvertFrom-Json).version
}
if ($Version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$') {
    throw 'Windows 发行版本须采用主版本.次版本.修订号，可附带预发布标识。'
}

$release = Join-Path $TargetDirectory 'release'
$exePath = Join-Path $release 'launcher-app.exe'
$installerName = "Lumi_$($Version)_x64-setup.exe"
$installerPath = Join-Path $release "bundle/nsis/$installerName"
$portableName = "Lumi_$($Version)_x64-portable.zip"

foreach ($file in @($exePath, $installerPath)) {
    if (!(Test-Path -LiteralPath $file -PathType Leaf) -or (Get-Item -LiteralPath $file).Length -eq 0) {
        throw "构建产物缺失或为空：$file"
    }
}

$signaturePath = "$installerPath.sig"
if (!$Unsigned) {
    $signature = (Get-Content -LiteralPath $signaturePath -Raw).Trim()
    $publicKey = ([string] $env:LUMI_UPDATER_PUBKEY).Trim()

    # Tauri 的 Base64 封装内是 Minisign 公钥和四行签名。
    try {
        $keyLines = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($publicKey)).TrimEnd() -split '\r?\n'
        $signatureLines = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($signature)).TrimEnd() -split '\r?\n'
        $keyBytes = [Convert]::FromBase64String($keyLines[1])
        $signatureBytes = [Convert]::FromBase64String($signatureLines[1])
        $commentSignature = [Convert]::FromBase64String($signatureLines[3])
    } catch {
        throw '更新器公钥或 Tauri 生成的安装器签名格式无效。'
    }

    if ($keyLines.Count -ne 2 -or $signatureLines.Count -ne 4 -or
        $keyBytes.Length -ne 42 -or $signatureBytes.Length -ne 74 -or $commentSignature.Length -ne 64) {
        throw '更新器公钥或 Tauri 生成的安装器签名格式无效。'
    }
    # Minisign 的标识位于两个算法字节之后。Tauri 遇到不匹配的密钥只会告警。
    if ([Convert]::ToHexString($keyBytes, 2, 8) -cne [Convert]::ToHexString($signatureBytes, 2, 8)) {
        throw '生成安装器签名的私钥与更新器公钥不匹配。'
    }
    $expectedComment = '^trusted comment: timestamp:[0-9]+\tfile:' + [regex]::Escape($installerName) + '\tversion:' + [regex]::Escape($Version) + '$'
    if ($signatureLines[2] -cnotmatch $expectedComment) {
        throw 'Tauri 生成的签名须绑定同名安装器及发行版本。'
    }
}

$output = [IO.Path]::GetFullPath($OutputDirectory)
if ((Test-Path -LiteralPath $output -PathType Leaf) -or
    ((Test-Path -LiteralPath $output) -and (Get-ChildItem -LiteralPath $output -Force))) {
    throw "请选择全新的或空的发行输出目录：$output"
}

$stage = Join-Path ([IO.Path]::GetDirectoryName($output)) ".lumi-release-$([Guid]::NewGuid().ToString('N'))"
[void] [IO.Directory]::CreateDirectory($stage)
try {
    Copy-Item -LiteralPath $installerPath -Destination $stage

    $zip = [IO.Compression.ZipFile]::Open((Join-Path $stage $portableName), [IO.Compression.ZipArchiveMode]::Create)
    try {
        [void] [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $exePath, 'Lumi.exe')
        [void] $zip.CreateEntry('Lumi.portable')

        $instructions = @'
Lumi 便携版
解压到可写目录，双击 Lumi.exe 运行。请保留旁边的 Lumi.portable。
启动器资料和默认游戏目录保存在旁边的 LumiData 文件夹。
手动更新时先退出 Lumi。可另行解压新版，再替换旧目录中的 Lumi.exe。
请保留原来的 Lumi.portable 和 LumiData。新版 ZIP 只包含程序、标记和说明。
'@
        $writer = [IO.StreamWriter]::new($zip.CreateEntry('README.txt').Open(), [Text.UTF8Encoding]::new($true))
        try {
            $writer.WriteLine($instructions)
        } finally {
            $writer.Dispose()
        }
    } finally {
        $zip.Dispose()
    }

    if (!$Unsigned) {
        Copy-Item -LiteralPath $signaturePath -Destination $stage
        @{
            version = $Version
            platforms = @{
                'windows-x86_64' = @{
                    url = "https://github.com/kokoa-77/LumiLauncher/releases/download/v$Version/$installerName"
                    signature = $signature
                }
            }
        } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $stage 'latest.json') -Encoding utf8NoBOM
    }

    $hashes = Get-ChildItem -LiteralPath $stage -File | Sort-Object Name | ForEach-Object {
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
        "$hash  $($_.Name)"
    }
    $lineFeed = [char] 10
    [IO.File]::WriteAllText((Join-Path $stage 'SHA256SUMS.txt'), ($hashes -join $lineFeed) + $lineFeed)

    if ([IO.Directory]::Exists($output)) {
        [IO.Directory]::Delete($output)
    }
    [IO.Directory]::Move($stage, $output)
} finally {
    if ([IO.Directory]::Exists($stage)) {
        Get-ChildItem -LiteralPath $stage -File -Force | Remove-Item
        [IO.Directory]::Delete($stage)
    }
}

Write-Host "Windows 发行文件已写入 $output"
