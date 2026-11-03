$ErrorActionPreference = 'Stop'

# The MSI keeps one UpgradeCode across versions; remove whichever version
# Windows Installer has for this account.
[array]$keys = Get-UninstallRegistryKey -SoftwareName 'Chat with Work Desktop'
foreach ($key in $keys) {
  if ($key.PSChildName -match '^\{[0-9A-Fa-f-]+\}$') {
    Uninstall-ChocolateyPackage -PackageName $env:ChocolateyPackageName -FileType 'msi' `
      -SilentArgs "$($key.PSChildName) /qn /norestart" -ValidExitCodes @(0, 3010, 1605, 1614, 1641)
  }
}
