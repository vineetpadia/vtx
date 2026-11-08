param(
    [string]$InstallDirectory = (Join-Path $env:LOCALAPPDATA 'VTXPreview'),
    [string]$BuildDirectory = (Join-Path $PSScriptRoot 'build'),
    [switch]$RegisterOpenWith,
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
# Windows PowerShell's hash/JSON helpers are script exports; load the matching
# built-in module explicitly when launched from a newer PowerShell environment.
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1')
$InstallDirectory = [IO.Path]::GetFullPath($InstallDirectory)
$backupPath = Join-Path $InstallDirectory 'preview-registration-backup.json'
$root = [Microsoft.Win32.Registry]::CurrentUser
$clsid = '{75660313-4B6C-45D7-96D9-39608F52F040}'
$category = '{8895b1c6-b41f-4c1c-a562-0d564250836f}'

function Notify-Explorer {
    if (-not ('VtxPreview.Associations' -as [type])) {
        Add-Type -TypeDefinition 'using System.Runtime.InteropServices; namespace VtxPreview { public static class Associations { [DllImport("shell32.dll")] public static extern void SHChangeNotify(uint e, uint f, System.IntPtr a, System.IntPtr b); } }'
    }
    [VtxPreview.Associations]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
}

if ($Uninstall) {
    if (-not (Test-Path -LiteralPath $backupPath)) { throw "No registration backup at $backupPath" }
    $state = Get-Content -LiteralPath $backupPath -Raw | ConvertFrom-Json
    $appKey = $root.OpenSubKey('Software\Classes\Applications\VTX.exe\shell\open\command')
    try { $ownsOpenWith = $null -ne $appKey -and $appKey.GetValue('') -ceq ('"' + (Join-Path $InstallDirectory 'VTX.exe') + '" "%1"') } finally { if ($appKey) { $appKey.Dispose() } }
    foreach ($entry in $state.Values) {
        # Older preview builds also registered Open with. A separately installed
        # main app owns those shared values now if its open command differs.
        $previewValue = $entry.Path -like 'Software\Classes\CLSID\*' -or $entry.Path -eq 'Software\Microsoft\Windows\CurrentVersion\PreviewHandlers' -or $entry.Path -like '*\shellex\*'
        if (-not $previewValue -and -not $ownsOpenWith) { continue }
        $key = $root.OpenSubKey($entry.Path, $true)
        if ($null -eq $key) { continue }
        try {
            # Preserve any association the user installed after VTX.
            if ($key.GetValue($entry.Name, $null) -cne $entry.Installed) { continue }
            if ($entry.Existed) {
                $key.SetValue($entry.Name, $entry.Value, [Microsoft.Win32.RegistryValueKind]$entry.Kind)
            } else { $key.DeleteValue($entry.Name, $false) }
        } finally { $key.Dispose() }
    }
    Notify-Explorer
    Write-Output 'Restored previous preview and Open with registrations. Installed files are retained so any existing preview host can finish safely.'
    exit
}

New-Item -ItemType Directory -Path $InstallDirectory -Force | Out-Null
$handlerSource = Join-Path $BuildDirectory 'VTXPreviewHD.dll'
if (-not (Test-Path -LiteralPath $handlerSource -PathType Leaf)) { throw "Build artifact missing: $handlerSource" }
$handlerName = 'VTXPreviewHD-' + (Get-FileHash -LiteralPath $handlerSource).Hash.Substring(0,12).ToLowerInvariant() + '.dll'
$rendererChanged = $false
foreach ($name in @('VTXPreviewHD.dll', 'VTX.exe')) {
    $source = Join-Path $BuildDirectory $name
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Build artifact missing: $source" }
    $destination = Join-Path $InstallDirectory $(if ($name -eq 'VTXPreviewHD.dll') { $handlerName } else { $name })
    if ((Test-Path -LiteralPath $destination) -and ((Get-FileHash -LiteralPath $source).Hash -eq (Get-FileHash -LiteralPath $destination).Hash)) { continue }
    if ($name -eq 'VTX.exe') { $rendererChanged = $true }
    if ($name -eq 'VTX.exe' -and (Test-Path -LiteralPath $destination)) {
        # This read-only preview worker has no unsaved document state. Close
        # only this installation's worker before replacing its executable.
        Get-Process -Name VTX -ErrorAction SilentlyContinue | Where-Object {
            try { $_.Path -ieq $destination } catch { $false }
        } | ForEach-Object {
            Stop-Process -InputObject $_ -Force
            if (-not $_.WaitForExit(5000)) { throw 'The previous preview worker did not exit. Close the Preview pane and try again.' }
        }
    }
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        try { Copy-Item -LiteralPath $source -Destination $destination -Force; break }
        catch [IO.IOException] {
            if ($attempt -eq 19) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}
if ($rendererChanged) {
    # Only regenerate this handler revision's disposable rendered images.
    $cacheDirectory = Join-Path $env:USERPROFILE 'AppData\LocalLow\VTX\Previews'
    if (Test-Path -LiteralPath $cacheDirectory -PathType Container) {
        Get-ChildItem -LiteralPath $cacheDirectory -Filter 'vtx-hd1920-v2-*.png' -File |
            Where-Object { $_.Name -notlike '*.tmp.png' } |
            ForEach-Object { Remove-Item -LiteralPath $_.FullName -ErrorAction SilentlyContinue }
    }
}
if ($PSScriptRoot -ne $InstallDirectory) {
    Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $InstallDirectory 'install.ps1') -Force
}
foreach ($name in @('LICENSE.libdeflate', 'README.md')) {
    $source = Join-Path $PSScriptRoot $name
    if ($PSScriptRoot -ne $InstallDirectory -and (Test-Path -LiteralPath $source)) {
        Copy-Item -LiteralPath $source -Destination (Join-Path $InstallDirectory $name) -Force
    }
}
$licenses = Join-Path $BuildDirectory 'licenses'
if (Test-Path -LiteralPath $licenses -PathType Container) {
    Copy-Item -LiteralPath $licenses -Destination $InstallDirectory -Recurse -Force
}
$values = @(
    @{Path="Software\Classes\CLSID\$clsid"; Name=''; Installed='VTX Preview'},
    @{Path="Software\Classes\CLSID\$clsid"; Name='DisplayName'; Installed='VTX Preview'},
    @{Path="Software\Classes\CLSID\$clsid"; Name='AppID'; Installed='{6d2b5079-2f0b-48dd-ab7f-97cec514d30b}'},
    @{Path="Software\Classes\CLSID\$clsid\InprocServer32"; Name=''; Installed=(Join-Path $InstallDirectory $handlerName)},
    @{Path="Software\Classes\CLSID\$clsid\InprocServer32"; Name='ThreadingModel'; Installed='Apartment'},
    @{Path='Software\Microsoft\Windows\CurrentVersion\PreviewHandlers'; Name=$clsid; Installed='VTX Preview'}
)
foreach ($extension in @('.sldprt','.sldasm','.step','.stp','.p21','.stl','.obj','.ply','.glb','.gltf','.fbx','.3mf','.ifc','.ifczip','.dxf')) {
    $values += @{Path="Software\Classes\$extension\shellex\$category"; Name=''; Installed=$clsid}
}
# Optionally offer the installed worker in Open with. The default preview-only
# install leaves the main app's existing Open-with registrations alone.
if ($RegisterOpenWith) {
# Offer the installed viewer in Open with without changing extension defaults
# or Windows' protected UserChoice values. Quote both executable and argument.
# https://learn.microsoft.com/windows/win32/shell/how-to-include-an-application-on-the-open-with-dialog-box
$viewer = Join-Path $InstallDirectory 'VTX.exe'
$openCommand = '"' + $viewer + '" "%1"'
$icon = '"' + $viewer + '",0'
$iconSource = Join-Path $BuildDirectory 'VTX.ico'
if (Test-Path -LiteralPath $iconSource -PathType Leaf) {
    $iconName = 'VTX-' + (Get-FileHash -LiteralPath $iconSource).Hash.Substring(0,12).ToLowerInvariant() + '.ico'
    $installedIcon = Join-Path $InstallDirectory $iconName
    Copy-Item -LiteralPath $iconSource -Destination $installedIcon -Force
    $icon = '"' + $installedIcon + '",0'
}
$application = 'Software\Classes\Applications\VTX.exe'
$progId = 'VTX.Model'
$capabilities = 'Software\VTX\Capabilities'
$values += @(
    @{Path=$application; Name='FriendlyAppName'; Installed='VTX'},
    @{Path="$application\DefaultIcon"; Name=''; Installed=$icon},
    @{Path="$application\shell\open\command"; Name=''; Installed=$openCommand},
    @{Path="Software\Classes\$progId"; Name=''; Installed='VTX model or drawing'},
    @{Path="Software\Classes\$progId\DefaultIcon"; Name=''; Installed=$icon},
    @{Path="Software\Classes\$progId\shell\open\command"; Name=''; Installed=$openCommand},
    @{Path="Software\Classes\$progId\Application"; Name='ApplicationName'; Installed='VTX'},
    @{Path="Software\Classes\$progId\Application"; Name='ApplicationDescription'; Installed='Fast local model and drawing viewer'},
    @{Path="Software\Classes\$progId\Application"; Name='ApplicationIcon'; Installed=$icon},
    @{Path=$capabilities; Name='ApplicationName'; Installed='VTX'},
    @{Path=$capabilities; Name='ApplicationDescription'; Installed='Fast local model and drawing viewer'},
    @{Path=$capabilities; Name='ApplicationIcon'; Installed=$icon},
    @{Path='Software\RegisteredApplications'; Name='VTX'; Installed=$capabilities}
)
foreach ($extension in @('.sldprt','.sldasm','.step','.stp','.p21','.stl','.obj','.ply','.glb','.gltf','.fbx','.3mf','.ifc','.ifczip','.dxf')) {
    $values += @{Path="$application\SupportedTypes"; Name=$extension; Installed=''}
    $values += @{Path="Software\Classes\$extension\OpenWithProgids"; Name=$progId; Installed=''}
    $values += @{Path="$capabilities\FileAssociations"; Name=$extension; Installed=$progId}
}
}
# Preserve originals, adding new registration keys and updating only the value
# we expect to own. This also supports migration from the first preview build.
$originals = @()
if (Test-Path -LiteralPath $backupPath) {
    $originals = @((Get-Content -LiteralPath $backupPath -Raw | ConvertFrom-Json).Values)
}
foreach ($entry in $values) {
    $original = $originals | Where-Object { $_.Path -eq $entry.Path -and $_.Name -eq $entry.Name } | Select-Object -First 1
    if ($null -ne $original) {
        $original.Installed = $entry.Installed
    } else {
        $key = $root.OpenSubKey($entry.Path)
        try {
            $entry.Existed = $null -ne $key -and $key.GetValueNames() -contains $entry.Name
            $entry.Value = if ($entry.Existed) { $key.GetValue($entry.Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) } else { $null }
            $entry.Kind = if ($entry.Existed) { [int]$key.GetValueKind($entry.Name) } else { 1 }
        } finally { if ($key) { $key.Dispose() } }
        $originals += $entry
    }
}
@{Version=2; Values=$originals} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$backupPath.tmp" -Encoding UTF8
Move-Item -LiteralPath "$backupPath.tmp" -Destination $backupPath -Force
foreach ($entry in $values) {
    $key = $root.CreateSubKey($entry.Path)
    try { $key.SetValue($entry.Name, $entry.Installed, [Microsoft.Win32.RegistryValueKind]::String) } finally { $key.Dispose() }
}
# Retire the old class without touching its loaded DLL or restarting Explorer.
foreach ($oldId in @('{A5C291A4-EC9E-4B80-B16B-8B94EBCA1193}', '{E74443FA-70BC-4A90-A5DA-5A4206F3C509}', '{F2494AF7-E5B4-433E-8FEA-A24FFEB1082E}', '{B16CA305-AC81-4A5C-9ECB-D7EE01505D38}', '{8912CE51-3F69-4314-8C12-6B62C6D80140}')) {
foreach ($entry in $originals | Where-Object { $_.Path -like "Software\Classes\CLSID\$oldId*" -or ($_.Path -eq 'Software\Microsoft\Windows\CurrentVersion\PreviewHandlers' -and $_.Name -eq $oldId) }) {
    $key = $root.OpenSubKey($entry.Path, $true)
    if ($key) {
        try {
            if (-not $entry.Existed -and $key.GetValue($entry.Name, $null) -ceq $entry.Installed) { $key.DeleteValue($entry.Name, $false) }
        } finally { $key.Dispose() }
    }
}
}
Notify-Explorer
Write-Output "Installed VTX Explorer Preview in $InstallDirectory. Enable Explorer's Preview pane with Alt+P. Existing default apps are unchanged."
