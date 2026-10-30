# Run only on an isolated Windows CI runner, after building the MSI.
param([Parameter(Mandatory)] [string] $Msi, [switch] $StandardUser, [string] $InstallFolder,
  [string] $PythonExecutable = "python",
  [string] $RuntimeSmoke = (Join-Path $PSScriptRoot "../../browser/engine/scripts/runtime_smoke.py"))
$ErrorActionPreference = "Stop"
if ($StandardUser) {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'This regression test must run without administrator rights'
  }
  if ($env:USERNAME -ne $identity.Name.Split('\')[-1]) {
    throw 'The test must use the standard account login environment'
  }
}
$Msi = (Resolve-Path $Msi).Path
$dir = if ($InstallFolder) { [System.IO.Path]::GetFullPath($InstallFolder) } else { Join-Path $env:LOCALAPPDATA "Programs\Chat with Work Desktop" }
$programs = [Environment]::GetFolderPath("Programs", "DoNotVerify")
$startup = Join-Path ([Environment]::GetFolderPath('Startup', 'DoNotVerify')) 'Chat with Work Desktop.lnk'
function Run-Msi([string] $action) {
  Write-Host "Running msiexec $action for $env:USERNAME"
  New-Item -ItemType Directory -Path $env:TEMP -Force | Out-Null
  $log = Join-Path $env:TEMP ('cww-msi-' + [Guid]::NewGuid().ToString('N') + '.log')
  $arguments = @($action, "`"$Msi`"", "/qn", "/norestart", '/L*V', "`"$log`"")
  if ($action -eq '/i' -and $InstallFolder) { $arguments += "INSTALLFOLDER=`"$dir`"" }
  $process = Start-Process msiexec.exe -Wait -PassThru -ArgumentList $arguments
  if ($process.ExitCode -notin @(0, 3010)) {
    if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log | Write-Host }
    throw "msiexec $action failed: $($process.ExitCode)"
  }
  Write-Host "Completed msiexec $action"
}
function Stop-Agent {
  $agentStatus = & (Join-Path $dir 'cww.exe') status --json | ConvertFrom-Json
  if ($LASTEXITCODE -ne 0 -or -not $agentStatus.pid) { throw 'Could not find the agent to stop' }
  $agentProcess = Get-Process -Id $agentStatus.pid -ErrorAction Stop
  try {
    & (Join-Path $dir 'cww.exe') daemon stop
    if ($LASTEXITCODE -ne 0) { throw 'Could not stop the agent' }
    # Shutdown closes the control pipe before all daemon threads exit.
    # A failed status request does not mean a new process can bind it yet.
    if (-not $agentProcess.WaitForExit(30000)) { throw 'The agent did not exit before the login test' }
  } finally {
    $agentProcess.Dispose()
  }
}
$installed = $false
try {
  Run-Msi "/i"
  $installed = $true
  foreach ($binary in "cww.exe", "cww-app.exe", "cww-agent.exe", "cww-scan-helper.exe", "TWAINDSM.dll", "TWAINDSM-LICENSE.txt", "TWAINDSM-NOTICE.txt") {
    if (-not (Test-Path (Join-Path $dir $binary))) { throw "Missing $binary" }
  }
  foreach ($shortcut in "Chat with Work.lnk", "Chat with Work (Terminal).lnk") {
    if (-not (Test-Path (Join-Path $programs $shortcut))) { throw "Missing $shortcut" }
  }
  & (Join-Path $dir "cww.exe") --version
  if ($LASTEXITCODE -ne 0) { throw "cww failed" }
  $engineRoot = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) "Programs\CwwBE"
  $locator = Get-Content (Join-Path $engineRoot "current.json") -Raw | ConvertFrom-Json
  if ($locator.protocol -ne 1 -or $locator.directory -notmatch '^[a-f0-9]{64}$') { throw "Invalid engine locator" }
  $engine = Join-Path $engineRoot $locator.directory
  if (-not (Test-Path (Join-Path $engine "manifest.json"))) { throw "Missing browser engine" }
  $manifest = Get-Content (Join-Path $engine "manifest.json") -Raw | ConvertFrom-Json
  if ((Get-FileHash (Join-Path $engine "manifest.json") -Algorithm SHA256).Hash.ToLowerInvariant() -ne $locator.directory) { throw "Engine identity differs" }
  foreach ($file in $manifest.files.PSObject.Properties) {
    $hash = (Get-FileHash (Join-Path $engine $file.Name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $file.Value) { throw "Installed engine differs: $($file.Name)" }
  }
  $installedFiles = @(Get-ChildItem $engine -Recurse -File -Force).Count
  if ($installedFiles -ne @($manifest.files.PSObject.Properties).Count + 1) { throw "Unexpected engine files" }
  & (Join-Path $engine "node.exe") --no-addons --no-global-search-paths --version
  if ($LASTEXITCODE -ne 0) { throw "Installed browser runtime did not start" }
  # Exercise native Known Folder discovery through this exact installed CLI,
  # including custom INSTALLFOLDER, without using the installed user's state.
  & $PythonExecutable $RuntimeSmoke --cww (Join-Path $dir "cww.exe")
  if ($LASTEXITCODE -ne 0) { throw "Installed browser engine did not become ready" }
  & (Join-Path $dir "cww-app.exe") --version
  if ($LASTEXITCODE -ne 0) { throw "cww-app failed" }
  # The 32-bit scanner helper starts and loads TWAINDSM.dll beside it: a
  # runner has no scanners, so it lists none rather than an error.
  $request = Join-Path $env:TEMP 'cww-scan-request.json'
  $answer = Join-Path $env:TEMP 'cww-scan-answer.json'
  Set-Content -LiteralPath $request -Value '{"op":"devices"}' -Encoding ascii
  $helper = Start-Process -FilePath (Join-Path $dir 'cww-scan-helper.exe') -PassThru -NoNewWindow `
    -RedirectStandardInput $request -RedirectStandardOutput $answer
  if (-not $helper.WaitForExit(60000)) { $helper.Kill(); throw 'The scanner helper did not answer' }
  $reply = Get-Content -LiteralPath $answer -Raw
  Write-Host "Scanner helper: $reply"
  if ($reply -notmatch '"devices"') { throw "The scanner helper did not list devices: $reply" }
  Remove-Item -LiteralPath $request, $answer
  # The MSI must actually start the per-user daemon, not merely ship it.
  $running = $false
  for ($i = 0; $i -lt 30; $i++) {
    $status = & (Join-Path $dir "cww.exe") status
    if ($status -match "running \(pid") { $running = $true; break }
    Start-Sleep -Seconds 1
  }
  if (-not $running) { throw "The MSI did not start the agent" }
  if ($StandardUser) {
    Write-Host 'Checking the startup shortcut'
    if (-not (Test-Path -LiteralPath $startup)) { throw 'No per-user startup fallback was created' }
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($startup)
    if ($shortcut.TargetPath -ne (Join-Path $dir 'cww-agent.exe')) { throw 'Startup points to the wrong agent' }
    # Exercise the same entry Explorer opens at login, after setup has exited.
    Stop-Agent
    # Custom paths must survive shortcut argument quoting, too. Set this
    # after the MSI check: deferred installer actions have their own environment.
    $env:CWW_HOME = Join-Path $env:LOCALAPPDATA 'Cww smoke test\state & data'
    Write-Host 'Registering the agent with a custom state path'
    & (Join-Path $dir 'cww.exe') daemon install
    if ($LASTEXITCODE -ne 0) { throw 'Could not register the custom state path' }
    Stop-Agent
    Start-Process -FilePath $startup
    Write-Host 'Opened the login shortcut'
    $running = $false
    for ($i = 0; $i -lt 30; $i++) {
      $status = & (Join-Path $dir 'cww.exe') status
      if ($status -match 'running \(pid') { $running = $true; break }
      Start-Sleep -Seconds 1
    }
    if (-not $running) { throw 'The login shortcut did not start the agent' }
    # The desktop app's Start Background Agent action must also be repeatable.
    & (Join-Path $dir 'cww.exe') daemon install
    if ($LASTEXITCODE -ne 0) { throw 'Starting the agent again as a standard user failed' }
  }
  # Uninstalling closes the running app and removes what the agent kept
  # (shared folders, the index, the log, keys) and the app's web view data.
  # The settings window is a web view, so the app's web view data exists too.
  $appProcess = Start-Process -FilePath (Join-Path $dir 'cww-app.exe') -ArgumentList '--settings' -PassThru
  $webData = Join-Path $env:LOCALAPPDATA 'com.chatwithwork.desktop'
  for ($i = 0; $i -lt 30 -and -not (Test-Path -LiteralPath $webData); $i++) { Start-Sleep -Seconds 1 }
  if ($appProcess.HasExited) { throw "The desktop app didn't stay running before the uninstall test: $($appProcess.ExitCode)" }
  if (-not (Test-Path -LiteralPath $webData)) { throw 'The app made no web view data to remove' }
  if (-not (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'cww'))) { throw 'The agent kept nothing to remove' }
} catch {
  if ($env:CWW_HOME) {
    $agentLog = Join-Path $env:CWW_HOME 'state\daemon.log'
    if (Test-Path -LiteralPath $agentLog) { Get-Content -LiteralPath $agentLog -Tail 100 | Write-Host }
  }
  throw
} finally {
  if ($StandardUser -and $env:CWW_HOME) {
    Write-Host 'Removing the custom agent'
    & (Join-Path $dir 'cww.exe') daemon uninstall
    Remove-Item Env:CWW_HOME
  }
  if ($installed) { Run-Msi "/x" }
}
foreach ($binary in "cww.exe", "cww-app.exe", "cww-agent.exe") {
  if (Test-Path (Join-Path $dir $binary)) { throw "Uninstall left $binary" }
}
if (Test-Path (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) "Programs\CwwBE")) { throw "Uninstall left the browser engine" }
foreach ($shortcut in "Chat with Work.lnk", "Chat with Work (Terminal).lnk") {
  if (Test-Path (Join-Path $programs $shortcut)) { throw "Uninstall left $shortcut" }
}
if (Test-Path -LiteralPath $startup) { throw 'Uninstall left the startup shortcut' }
if ($appProcess -and -not $appProcess.WaitForExit(15000)) { $appProcess.Kill(); throw 'Uninstall left the desktop app running' }
foreach ($data in (Join-Path $env:LOCALAPPDATA 'cww'), (Join-Path $env:LOCALAPPDATA 'com.chatwithwork.desktop')) {
  if (Test-Path -LiteralPath $data) { throw "Uninstall left $data" }
}
