<#
.SYNOPSIS
  Install Nocturne for Windows x64 from the latest GitHub Release.

.DESCRIPTION
  Downloads the latest nocturne-*-windows-x64-portable.zip and SHA256SUMS.txt from
  https://github.com/antimajorana/Nocturne/releases/latest, verifies the SHA-256
  checksum, installs to $env:LOCALAPPDATA\Programs\Nocturne, adds that folder to
  the user PATH, and creates Desktop and Start Menu shortcuts.

.EXAMPLE
  irm https://raw.githubusercontent.com/antimajorana/Nocturne/main/packaging/install.ps1 | iex
#>
$ErrorActionPreference = 'Stop'

if ($env:OS -ne 'Windows_NT') {
  throw 'This installer is for Windows x64. On macOS or Linux, use packaging/install.sh.'
}

$Repo = 'antimajorana/Nocturne'
$ApiUrl = "https://api.github.com/repos/$Repo/releases/latest"
$Headers = @{
  'User-Agent' = 'Nocturne-Installer'
  'Accept'     = 'application/vnd.github+json'
}

# Ensure TLS 1.2+ on Windows PowerShell 5.1.
$protocols = [Net.SecurityProtocolType]::Tls12
if ([Enum]::GetNames([Net.SecurityProtocolType]) -contains 'Tls13') {
  $protocols = $protocols -bor [Net.SecurityProtocolType]::Tls13
}
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor $protocols

$prevProgress = $ProgressPreference
$ProgressPreference = 'SilentlyContinue'
$TmpDir = Join-Path ([IO.Path]::GetTempPath()) ("nocturne-install-" + [Guid]::NewGuid().ToString('N'))

try {
  Write-Output "Checking latest Nocturne release from $Repo..."
  try {
    $release = Invoke-RestMethod -Uri $ApiUrl -Headers $Headers
  }
  catch {
    $statusCode = $null
    if ($_.Exception.Response -and $_.Exception.Response.StatusCode) {
      $statusCode = [int] $_.Exception.Response.StatusCode
    }
    if ($statusCode -eq 404) {
      throw "No published Nocturne release exists yet at https://github.com/$Repo/releases."
    }
    throw "Failed to query $ApiUrl`: $($_.Exception.Message)"
  }

  if (-not $release -or -not $release.assets) {
    throw "No release assets found in the latest release from https://github.com/$Repo/releases."
  }

  $zipAsset = $release.assets | Where-Object { $_.name -match '^nocturne-.*-windows-x64-portable\.zip$' } | Select-Object -First 1
  $sumsAsset = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS.txt' } | Select-Object -First 1

  if (-not $zipAsset) {
    throw "Latest release ($($release.tag_name)) does not contain a windows-x64-portable.zip asset."
  }
  if (-not $sumsAsset) {
    throw "Latest release ($($release.tag_name)) does not contain a SHA256SUMS.txt asset."
  }

  New-Item -ItemType Directory -Force -Path $TmpDir | Out-Null
  $zipPath = Join-Path $TmpDir $zipAsset.name
  $sumsPath = Join-Path $TmpDir 'SHA256SUMS.txt'

  Write-Output "Downloading $($zipAsset.name) ($($release.tag_name))..."
  Invoke-WebRequest -Uri $zipAsset.browser_download_url -Headers $Headers -OutFile $zipPath -UseBasicParsing
  Write-Output 'Downloading SHA256SUMS.txt...'
  Invoke-WebRequest -Uri $sumsAsset.browser_download_url -Headers $Headers -OutFile $sumsPath -UseBasicParsing

  $expectedHash = $null
  foreach ($line in Get-Content -LiteralPath $sumsPath -Encoding UTF8) {
    if ($line.Trim() -match '^([0-9a-fA-F]{64})\s+\*?(.+)$') {
      if ($Matches[2].Trim() -eq $zipAsset.name) {
        $expectedHash = $Matches[1].ToLowerInvariant()
        break
      }
    }
  }
  if (-not $expectedHash) {
    throw "SHA256SUMS.txt does not list an entry for $($zipAsset.name)."
  }

  $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zipPath).Hash.ToLowerInvariant()
  if ($actualHash -ne $expectedHash) {
    throw "SHA-256 verification failed for $($zipAsset.name)! Expected $expectedHash, got $actualHash."
  }
  Write-Output "Verified SHA-256: $actualHash"

  $extractDir = Join-Path $TmpDir 'extracted'
  Expand-Archive -LiteralPath $zipPath -DestinationPath $extractDir -Force

  $payloadRoot = $null
  if (Test-Path (Join-Path $extractDir 'nocturne.exe') -PathType Leaf) {
    $payloadRoot = $extractDir
  }
  else {
    $candidates = @(Get-ChildItem -LiteralPath $extractDir -Directory)
    foreach ($dir in $candidates) {
      if (Test-Path (Join-Path $dir.FullName 'nocturne.exe') -PathType Leaf) {
        $payloadRoot = $dir.FullName
        break
      }
    }
  }
  if (-not $payloadRoot) {
    throw "Extracted archive does not contain nocturne.exe."
  }

  $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Nocturne'
  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

  foreach ($staleSubdir in 'demo', 'importers', 'licenses') {
    $targetSub = Join-Path $InstallDir $staleSubdir
    if (Test-Path $targetSub) {
      Remove-Item -LiteralPath $targetSub -Recurse -Force
    }
  }
  Get-ChildItem -LiteralPath $payloadRoot -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $InstallDir -Recurse -Force
  }

  $exePath = Join-Path $InstallDir 'nocturne.exe'
  if (-not (Test-Path $exePath -PathType Leaf)) {
    throw "Installation failed: $exePath was not found after copying files."
  }

  $normalizedInstall = $InstallDir.TrimEnd('\')
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $pathEntries = @()
  if ($userPath) {
    $pathEntries = @($userPath -split ';' | Where-Object { $_.Trim() -ne '' })
  }
  $alreadyOnUserPath = $false
  foreach ($entry in $pathEntries) {
    if ($entry.Trim().TrimEnd('\') -ieq $normalizedInstall) {
      $alreadyOnUserPath = $true
      break
    }
  }
  if (-not $alreadyOnUserPath) {
    $newUserPath = (@($pathEntries) + @($InstallDir)) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
    $pathAdded = $true
  }
  else {
    $pathAdded = $false
  }

  $sessionEntries = @($env:Path -split ';' | Where-Object { $_.Trim() -ne '' })
  $alreadyOnSessionPath = $false
  foreach ($entry in $sessionEntries) {
    if ($entry.Trim().TrimEnd('\') -ieq $normalizedInstall) {
      $alreadyOnSessionPath = $true
      break
    }
  }
  if (-not $alreadyOnSessionPath) {
    $env:Path = "$env:Path;$InstallDir"
  }

  $desktopLink = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Nocturne.lnk'
  $startMenuDir = [Environment]::GetFolderPath('Programs')
  New-Item -ItemType Directory -Force -Path $startMenuDir | Out-Null
  $startMenuLink = Join-Path $startMenuDir 'Nocturne.lnk'

  $wsh = New-Object -ComObject WScript.Shell
  try {
    foreach ($linkPath in @($desktopLink, $startMenuLink)) {
      $shortcut = $wsh.CreateShortcut($linkPath)
      $shortcut.TargetPath = $exePath
      $shortcut.WorkingDirectory = $InstallDir
      $shortcut.IconLocation = "$exePath,0"
      $shortcut.Description = 'Nocturne note taking'
      $shortcut.Save()
    }
  }
  finally {
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($wsh)
  }

  Write-Output ''
  Write-Output "Nocturne $($release.tag_name) installed successfully."
  Write-Output "  Install directory : $InstallDir"
  Write-Output "  Executable        : $exePath"
  Write-Output "  Desktop shortcut  : $desktopLink"
  Write-Output "  Start Menu        : $startMenuLink"
  if ($pathAdded) {
    Write-Output "  User PATH         : added $InstallDir (open a new terminal to use 'nocturne' / 'nocturne-cli' everywhere)"
  }
  else {
    Write-Output "  User PATH         : already includes $InstallDir"
  }
}
finally {
  $ProgressPreference = $prevProgress
  if (Test-Path $TmpDir) {
    Remove-Item -LiteralPath $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
  }
}
