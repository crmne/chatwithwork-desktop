$ErrorActionPreference = 'Stop'

# The MSI keeps one UpgradeCode across versions; remove whichever version
# Windows Installer has for this account.
[array]$keys = Get-UninstallRegistryKey -SoftwareName 'Chat with Work Desktop'
if ($keys.Count -eq 0) {
  # The MSI installs per user: another account's copy isn't visible here.
  Write-Warning "Chat with Work Desktop isn't installed for $env:USERNAME. Sign in as the account that installed it and run choco uninstall chatwithwork-desktop there, or remove it in Settings > Apps."
}
foreach ($key in $keys) {
  if ($key.PSChildName -match '^\{[0-9A-Fa-f-]+\}$') {
    Uninstall-ChocolateyPackage -PackageName $env:ChocolateyPackageName -FileType 'msi' `
      -SilentArgs "$($key.PSChildName) /qn /norestart" -ValidExitCodes @(0, 3010, 1605, 1614, 1641)
  }
}
