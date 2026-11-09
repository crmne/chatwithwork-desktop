# Install Chat with Work Desktop (cww) on Windows from the latest
# GitHub release, without administrator rights.
#
#   powershell -ExecutionPolicy Bypass -c "irm https://github.com/crmne/chatwithwork-desktop/releases/latest/download/cww-installer.ps1 | iex"
#
# Downloads the MSI for this machine (x64 or arm64), checks it against the
# release's checksums.txt, and installs it for the current user: cww.exe on
# the PATH, the GUI and terminal Start menu entries, and the daemon's logon task, idle until you
# pair. Set $env:CWW_VERSION = "1.2.3" for a specific release, or
# $env:CWW_DOWNLOAD_BASE to a mirror holding a release's files.
$ErrorActionPreference = "Stop"
$repo = "crmne/chatwithwork-desktop"

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
  "AMD64" { "x86_64-pc-windows-msvc" }
  "ARM64" { "aarch64-pc-windows-msvc" }
  default { throw "No build for $($env:PROCESSOR_ARCHITECTURE) Windows" }
}
if ($env:CWW_DOWNLOAD_BASE) {
  # A mirror, or a local copy of a release for testing.
  $base = $env:CWW_DOWNLOAD_BASE.TrimEnd("/")
} elseif ($env:CWW_VERSION) {
  $base = "https://github.com/$repo/releases/download/v$($env:CWW_VERSION.TrimStart('v'))"
} else {
  $base = "https://github.com/$repo/releases/latest/download"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("cww-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $ProgressPreference = "SilentlyContinue"
  $sums = Join-Path $tmp "checksums.txt"
  Invoke-WebRequest -UseBasicParsing "$base/checksums.txt" -OutFile $sums
  $line = Select-String -Path $sums -Pattern "cww-v\S+-$arch\.msi$" | Select-Object -First 1
  if (-not $line) { throw "The release has no installer for $arch" }
  $expected, $asset = ($line.Line -split "\s+", 2)
  $asset = $asset.Trim()
  Write-Host "Downloading $asset"
  $msi = Join-Path $tmp $asset
  Invoke-WebRequest -UseBasicParsing "$base/$asset" -OutFile $msi
  $actual = (Get-FileHash -Algorithm SHA256 $msi).Hash
  if ($actual -ne $expected.ToUpper()) { throw "Checksum mismatch for $asset" }

  $log = Join-Path $tmp "install.log"
  $process = Start-Process msiexec.exe -Wait -PassThru `
    -ArgumentList "/i", "`"$msi`"", "/qb!", "/norestart", "/l*v", "`"$log`""
  if ($process.ExitCode -ne 0) { throw "msiexec failed ($($process.ExitCode)); see $log" }

  $dir = Join-Path $env:LOCALAPPDATA "Programs\Chat with Work Desktop"
  $env:Path = "$env:Path;$dir"
  $cww = Join-Path $dir 'cww.exe'
  Write-Host ""
  Write-Host "Installed $(& $cww --version) to $dir"

  # The MSI goes on when the agent doesn't start, so the app can offer to
  # start it later: say so here instead of reporting success.

  # "yes" when the agent answers `status` with its process, "no" when
  # `cww status` says none is running, and "unknown" otherwise: it didn't
  # answer within $waitMs, failed, or couldn't be started.
  function Get-AgentState([int]$waitMs = 10000) {
    $out = Join-Path $tmp "status.json"
    try {
      $probe = Start-Process -FilePath $cww -ArgumentList "status", "--json" -NoNewWindow -PassThru `
        -RedirectStandardOutput $out -RedirectStandardError (Join-Path $tmp "status.err")
      # Windows PowerShell keeps the exit code only once the handle was read.
      $null = $probe.Handle
    } catch {
      return "unknown"
    }
    if (-not $probe.WaitForExit($waitMs)) {
      Stop-Process -Id $probe.Id -Force -ErrorAction SilentlyContinue
      return "unknown"
    }
    if ($probe.ExitCode -ne 0) { return "unknown" }
    try { $status = Get-Content -Raw $out | ConvertFrom-Json } catch { return "unknown" }
    if ($null -ne $status.pid) { return "yes" }
    if ($status.running -eq $false) { return "no" }
    return "unknown"
  }
  # About 20 seconds in all, however long each try takes.
  $agent = "unknown"
  $clock = [Diagnostics.Stopwatch]::StartNew()
  while ($clock.ElapsedMilliseconds -lt 20000) {
    $agent = Get-AgentState ([Math]::Min(10000, [Math]::Max(1000, 20000 - $clock.ElapsedMilliseconds)))
    if ($agent -eq "yes") { break }
    Start-Sleep -Seconds 1
  }
  Write-Host ""
  if ($agent -eq "yes") {
    Write-Host "The background agent is running for your account."
    Write-Host "Open Chat with Work from the Start menu, or run cww for the terminal interface."
    Write-Host "Nothing is shared until you pair this computer and choose a folder."
  } else {
    if ($agent -eq "no") {
      Write-Warning "Chat with Work Desktop is installed, but its background agent is not running."
    } else {
      Write-Warning "Chat with Work Desktop is installed, but this installer couldn't tell whether its background agent is running."
    }
    # What the installer's own step said when it started it (cww daemon
    # install), which the log, deleted below, otherwise keeps to itself.
    $said = Select-String -Path $log -Pattern "WixQuietExec:\s+error: (.*)" -ErrorAction SilentlyContinue |
      Select-Object -Last 1
    if ($said) { Write-Warning ("Starting it during the install said: " + $said.Matches[0].Groups[1].Value.Trim()) }
    Write-Warning "To start it, open Chat with Work from the Start menu, or run: cww daemon install"
    Write-Warning "Check it with: cww status"
  }
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
