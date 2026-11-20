#Requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $OutputPath
)

$ErrorActionPreference = 'Stop'

$publicKey = ([string] $env:LUMI_UPDATER_PUBKEY).Trim()
if ($publicKey -cnotmatch '^[A-Za-z0-9+/]+={0,2}$') {
    throw 'LUMI_UPDATER_PUBKEY 必须是 Tauri signer 生成的 Base64 公钥内容。'
}

# 这个覆盖文件只传入更新公钥，不接触签名私钥或游戏登录配置。
@{
    bundle = @{
        createUpdaterArtifacts = $true
    }
    plugins = @{
        updater = @{
            pubkey = $publicKey
            requireSignedVersion = $true
        }
    }
} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding utf8NoBOM
