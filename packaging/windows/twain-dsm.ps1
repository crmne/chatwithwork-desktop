# Put the TWAIN Working Group's 32-bit TWAINDSM.dll (release 2.5.1) in a
# folder, for the 32-bit scanner helper beside it. Downloaded from the TWAIN
# Working Group's own release and checked against fixed SHA-256 hashes, so
# a build never ships a different file.
#
#   pwsh packaging/windows/twain-dsm.ps1 -Destination target\release
#
# It's LGPL-2.1: TWAINDSM-LICENSE.txt and TWAINDSM-NOTICE.txt, beside this
# script, ship with it.
param([Parameter(Mandatory)] [string] $Destination)
$ErrorActionPreference = "Stop"
$url = "https://github.com/twain/twain-dsm/releases/download/v2.5.1/dsm_020501.zip"
$zipHash = "6d5b520e0d4d524a195aa6ed749edb09d7118eed96c7defb4952bfcdb1590cee"
$dllHash = "cee9a6ff8026dde09eda9f892c57cc928d42e4a4d22bb740fe38ee6e91797a28"

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("cww-twain-dsm-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
try {
  $zip = Join-Path $work "dsm.zip"
  Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
  $got = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
  if ($got -ne $zipHash) { throw "dsm_020501.zip has SHA-256 $got, not $zipHash" }
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
  try {
    $entry = $archive.Entries | Where-Object { $_.FullName -eq "dsm_020501/windows/32/TWAINDSM.dll" }
    if (-not $entry) { throw "dsm_020501.zip has no windows/32/TWAINDSM.dll" }
    $dll = Join-Path $work "TWAINDSM.dll"
    [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $dll)
  } finally {
    $archive.Dispose()
  }
  $got = (Get-FileHash -Algorithm SHA256 $dll).Hash.ToLowerInvariant()
  if ($got -ne $dllHash) { throw "TWAINDSM.dll has SHA-256 $got, not $dllHash" }
  New-Item -ItemType Directory -Force -Path $Destination | Out-Null
  Copy-Item $dll (Join-Path $Destination "TWAINDSM.dll") -Force
  Write-Host "TWAINDSM.dll 2.5.1 (32-bit) in $Destination"
} finally {
  Remove-Item -Recurse -Force $work
}
