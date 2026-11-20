param([Parameter(Mandatory)][string]$OutputPath)
$ErrorActionPreference='Stop'
$framework=Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319'
$csc=Join-Path $framework 'csc.exe'
if (!(Test-Path -LiteralPath $csc -PathType Leaf)) { throw 'The .NET Framework Windows compiler was not found.' }
$refs=@('PresentationCore.dll','PresentationFramework.dll','WindowsBase.dll') | ForEach-Object { "/reference:$(Join-Path $framework "WPF\$_")" }
$icon=Join-Path $PSScriptRoot '..\..\icons\icon.ico'
$assets=@(
  "/resource:$(Join-Path $PSScriptRoot 'SetupWindow.xaml'),SetupWindow.xaml",
  "/resource:$(Join-Path $PSScriptRoot 'setup-night.jpg'),setup-night.jpg",
  "/resource:$icon,lumi.ico"
)
& $csc /nologo /target:winexe /platform:anycpu /optimize+ /utf8output "/out:$OutputPath" "/win32icon:$icon" /reference:System.Xaml.dll /reference:System.Windows.Forms.dll /reference:System.Web.Extensions.dll $refs $assets (Join-Path $PSScriptRoot 'SetupWindow.cs')
if ($LASTEXITCODE) { exit $LASTEXITCODE }
