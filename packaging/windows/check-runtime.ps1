# Fail when a Windows executable needs the Visual C++ Redistributable.
#
#   pwsh packaging/windows/check-runtime.ps1 target\release\cww.exe ...
#
# A fresh Windows install has the Universal CRT (ucrtbase.dll and the
# api-ms-win-crt-* forwarders) but not VCRUNTIME140.dll or MSVCP140.dll, so
# the binaries must link the C runtime statically (.cargo/config.toml).
param([Parameter(Mandatory, ValueFromRemainingArguments)] [string[]] $Paths)
$ErrorActionPreference = "Stop"

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$dumpbin = & $vswhere -latest -products * -find "VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe" | Select-Object -First 1
if (-not $dumpbin) { throw "dumpbin.exe not found" }

$failed = $false
foreach ($path in $Paths) {
  $imports = & $dumpbin /nologo /dependents $path
  if ($LASTEXITCODE -ne 0) { throw "dumpbin failed on $path" }
  $redist = $imports | Where-Object { $_ -match "^\s*(vcruntime|msvcp|concrt)\d+.*\.dll\s*$" } | ForEach-Object { $_.Trim() }
  if ($redist) {
    Write-Host "::error::$path needs the Visual C++ Redistributable: $($redist -join ', ')"
    $failed = $true
  } else {
    Write-Host "$path links the C runtime statically"
  }
}
if ($failed) { exit 1 }
