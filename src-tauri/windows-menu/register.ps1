# The caller passes paths as arguments, never interpolated PowerShell source.
param(
    [Parameter(Mandatory = $true)][ValidateSet('Enable', 'Disable')][string]$Action,
    [Parameter(Mandatory = $true)][string]$ManifestPath,
    [Parameter(Mandatory = $true)][string]$ExternalLocation,
    [switch]$ElevatedRegistration,
    [string]$OriginalUserSid,
    [string]$ErrorFile
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
# The app may have been launched from PowerShell 7. Its PSModulePath is not
# necessarily usable by Windows PowerShell 5.1 (which supplies the Appx module).
$env:PSModulePath = "$PSHOME\Modules;$env:SystemRoot\System32\WindowsPowerShell\v1.0\Modules;$env:PSModulePath"
function Get-ManifestHash([string]$Path) {
    $hash = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($hash.ComputeHash([IO.File]::ReadAllBytes($Path))).Replace('-', '') }
    finally { $hash.Dispose() }
}

function Register-Menu {
    # Only this identity-only package is removed; application data uses a
    # different identity and is never passed to Remove-AppxPackage.
    $existing = Get-AppxPackage -Name $name
    if ($existing) { $existing | Remove-AppxPackage }
    Add-AppxPackage -Path $ManifestPath -Register -ExternalLocation $ExternalLocation
    $registered = Get-AppxPackage -Name $name
    if (!$registered) { throw 'The Windows 11 menu package was not registered.' }
    @{ identity = $registered.PackageFullName; content = $wanted } |
        ConvertTo-Json | Set-Content -LiteralPath $marker -Encoding UTF8
}

function Register-WithTemporaryDeveloperMode {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if ($identity.User.Value -ne $OriginalUserSid) {
        throw 'Approve UAC with the same Windows account that is running the application.'
    }
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Administrator permission is required for this registration.'
    }
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
        [Microsoft.Win32.RegistryHive]::LocalMachine, [Microsoft.Win32.RegistryView]::Registry64)
    $path = 'SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock'
    $valueName = 'AllowDevelopmentWithoutDevLicense'
    $key = $null
    $changed = $false
    $keyPresent = $false
    $valuePresent = $false
    try {
        $key = $base.OpenSubKey($path, $true)
        $keyPresent = $null -ne $key
        if ($key) {
            $valuePresent = $key.GetValueNames() -contains $valueName
            if ($valuePresent) {
                $oldValue = $key.GetValue($valueName)
                $oldKind = $key.GetValueKind($valueName)
            }
        }
        if (!$key) { $key = $base.CreateSubKey($path) }
        $changed = $true
        $key.SetValue($valueName, 1, [Microsoft.Win32.RegistryValueKind]::DWord)
        Register-Menu
    } finally {
        try {
            if ($changed) {
                if ($valuePresent) { $key.SetValue($valueName, $oldValue, $oldKind) }
                else { $key.DeleteValue($valueName, $false) }
                # Only remove a key we created, and only if it is still empty.
                if (!$keyPresent -and $key.GetValueNames().Count -eq 0 -and $key.GetSubKeyNames().Count -eq 0) {
                    $key.Dispose()
                    $key = $null
                    $base.DeleteSubKey($path)
                }
            }
        } finally {
            if ($key) { $key.Dispose() }
            $base.Dispose()
        }
    }
}

function Request-Registration {
    $errorPath = Join-Path (Split-Path -Parent $ManifestPath) (([Guid]::NewGuid().ToString()) + '.error')
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    # These are file paths (Windows paths cannot contain quotes), passed to
    # -File rather than constructed PowerShell source. Quote each argument for
    # Start-Process, which joins ArgumentList into one Windows command line.
    $arguments = @('-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        '-File', $PSCommandPath, '-Action', 'Enable', '-ManifestPath', $ManifestPath,
        '-ExternalLocation', $ExternalLocation.TrimEnd('\'), '-ElevatedRegistration',
        '-OriginalUserSid', $sid, '-ErrorFile', $errorPath) |
        ForEach-Object { '"' + $_ + '"' }
    try {
        $process = Start-Process -FilePath "$PSHOME\powershell.exe" -ArgumentList $arguments `
            -Verb RunAs -WindowStyle Hidden -Wait -PassThru
        if ($process.ExitCode -ne 0) {
            if ([IO.File]::Exists($errorPath)) { throw [IO.File]::ReadAllText($errorPath) }
            throw 'Windows 11 menu registration was cancelled or failed.'
        }
    } finally {
        if ([IO.File]::Exists($errorPath)) { [IO.File]::Delete($errorPath) }
    }
}
$name = 'ScrcpyAgent.ExplorerMenu'
$installed = Get-AppxPackage -Name $name
if ($Action -eq 'Disable') {
    if ($installed) { $installed | Remove-AppxPackage }
    exit 0
}
$marker = Join-Path (Split-Path -Parent $ManifestPath) 'registration.json'
$wanted = (Get-ManifestHash $ManifestPath) + '|' + $ExternalLocation
if ($installed -and (Test-Path -LiteralPath $marker)) {
    $previous = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
    if ($previous.identity -eq $installed.PackageFullName -and $previous.content -eq $wanted) { exit 0 }
}
# A verified registration can be reused after Developer Mode has been restored.
# The source folder can differ when the developer verifies a build before the
# running app has deployed its own copy of these resources.
if ($installed -and $installed.InstallLocation) {
    $installedMarker = Join-Path $installed.InstallLocation 'registration.json'
    if (Test-Path -LiteralPath $installedMarker) {
        $previous = Get-Content -LiteralPath $installedMarker -Raw | ConvertFrom-Json
        if ($previous.identity -eq $installed.PackageFullName -and $previous.content -eq $wanted) { exit 0 }
    }
}
try {
    if ($ElevatedRegistration) {
        Register-WithTemporaryDeveloperMode
    } else {
        $mode = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock' `
            -Name AllowDevelopmentWithoutDevLicense -ErrorAction SilentlyContinue
        if ($mode -and $mode.AllowDevelopmentWithoutDevLicense -eq 1) { Register-Menu }
        else { Request-Registration }
    }
} catch {
    if ($ElevatedRegistration -and $ErrorFile) {
        [IO.File]::WriteAllText($ErrorFile, $_.ToString(), (New-Object System.Text.UTF8Encoding($false)))
    }
    Write-Error $_
    exit 1
}
