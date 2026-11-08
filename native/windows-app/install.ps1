param(
    [string]$ExecutableSource = (Join-Path $PSScriptRoot '..\target\release\examples\vtx.exe'),
    [string]$InstallDirectory = (Join-Path $env:LOCALAPPDATA 'VTX'),
    [string]$LicensesDirectory = (Join-Path $PSScriptRoot '..\..\..\app\licenses'),
    [switch]$Uninstall
)
$ErrorActionPreference='Stop'
$InstallDirectory=[IO.Path]::GetFullPath($InstallDirectory)
$backup=Join-Path $InstallDirectory 'app-registration-backup.json'
$root=[Microsoft.Win32.Registry]::CurrentUser
if($Uninstall){
    $state=Get-Content -LiteralPath $backup -Raw | ConvertFrom-Json
    foreach($entry in $state.Values){
        $key=$root.OpenSubKey($entry.Path,$true);if(-not $key){continue}
        try{if($key.GetValue($entry.Name,$null) -cne $entry.Installed){continue};if($entry.Existed){$key.SetValue($entry.Name,$entry.Value,[Microsoft.Win32.RegistryValueKind]$entry.Kind)}else{$key.DeleteValue($entry.Name,$false)}}finally{$key.Dispose()}
    }
}else{
    New-Item -ItemType Directory -Path $InstallDirectory -Force | Out-Null
    $viewer=Join-Path $InstallDirectory 'VTX.exe'
    if(-not (Test-Path -LiteralPath $ExecutableSource -PathType Leaf)){throw 'Build the VTX desktop example first.'}
    if(-not (Test-Path -LiteralPath $viewer) -or (Get-FileHash -LiteralPath $viewer).Hash -ne (Get-FileHash -LiteralPath $ExecutableSource).Hash){Copy-Item -LiteralPath $ExecutableSource -Destination $viewer -Force}
    if(Test-Path -LiteralPath $LicensesDirectory){Copy-Item -LiteralPath $LicensesDirectory -Destination $InstallDirectory -Recurse -Force}
    if($PSCommandPath -ine (Join-Path $InstallDirectory 'install.ps1')){Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $InstallDirectory 'install.ps1') -Force}
    $command='"'+$viewer+'" "%1"';$icon='"'+$viewer+'",0'
    $application='Software\Classes\Applications\VTX.exe';$progId='VTX.Model';$capabilities='Software\VTX\Capabilities'
    $values=@(
        @{Path=$application;Name='FriendlyAppName';Installed='VTX'},
        @{Path="$application\DefaultIcon";Name='';Installed=$icon},
        @{Path="$application\shell\open\command";Name='';Installed=$command},
        @{Path="Software\Classes\$progId";Name='';Installed='VTX model or drawing'},
        @{Path="Software\Classes\$progId\DefaultIcon";Name='';Installed=$icon},
        @{Path="Software\Classes\$progId\shell\open\command";Name='';Installed=$command},
        @{Path="Software\Classes\$progId\Application";Name='ApplicationName';Installed='VTX'},
        @{Path="Software\Classes\$progId\Application";Name='ApplicationDescription';Installed='Fast local model inspection'},
        @{Path="Software\Classes\$progId\Application";Name='ApplicationIcon';Installed=$icon},
        @{Path=$capabilities;Name='ApplicationName';Installed='VTX'},
        @{Path=$capabilities;Name='ApplicationDescription';Installed='Fast local model inspection'},
        @{Path=$capabilities;Name='ApplicationIcon';Installed=$icon},
        @{Path='Software\RegisteredApplications';Name='VTX';Installed=$capabilities}
    )
    foreach($ext in @('.sldprt','.sldasm','.step','.stp','.p21','.stl','.obj','.ply','.glb','.gltf','.fbx','.3mf','.ifc','.ifczip','.dxf')){
        $values+=@{Path="$application\SupportedTypes";Name=$ext;Installed=''}
        $values+=@{Path="Software\Classes\$ext\OpenWithProgids";Name=$progId;Installed=''}
        $values+=@{Path="$capabilities\FileAssociations";Name=$ext;Installed=$progId}
    }
    $originals=@();if(Test-Path -LiteralPath $backup){$originals=@((Get-Content -LiteralPath $backup -Raw|ConvertFrom-Json).Values)}
    foreach($entry in $values){
        $original=$originals|Where-Object{$_.Path -eq $entry.Path -and $_.Name -eq $entry.Name}|Select-Object -First 1
        if($original){$original.Installed=$entry.Installed}else{
            $key=$root.OpenSubKey($entry.Path)
            try{$entry.Existed=$null -ne $key -and $key.GetValueNames() -contains $entry.Name;$entry.Value=if($entry.Existed){$key.GetValue($entry.Name)}else{$null};$entry.Kind=if($entry.Existed){[int]$key.GetValueKind($entry.Name)}else{1}}finally{if($key){$key.Dispose()}}
            $originals+=$entry
        }
    }
    @{Version=1;Values=$originals}|ConvertTo-Json -Depth 5|Set-Content -LiteralPath "$backup.tmp" -Encoding UTF8
    Move-Item -LiteralPath "$backup.tmp" -Destination $backup -Force
    foreach($entry in $values){$key=$root.CreateSubKey($entry.Path);try{$key.SetValue($entry.Name,$entry.Installed,[Microsoft.Win32.RegistryValueKind]::String)}finally{$key.Dispose()}}
    $shortcutPath=Join-Path ([Environment]::GetFolderPath('Programs')) 'VTX.lnk'
    if(-not (Test-Path -LiteralPath $shortcutPath)){$shell=New-Object -ComObject WScript.Shell;$shortcut=$shell.CreateShortcut($shortcutPath);$shortcut.TargetPath=$viewer;$shortcut.WorkingDirectory=$InstallDirectory;$shortcut.Description='VTX model viewer';$shortcut.Save()}
}
if(-not ('VtxApp.Notice' -as [type])){Add-Type 'using System;using System.Runtime.InteropServices;namespace VtxApp{public static class Notice{[DllImport("shell32.dll")]public static extern void SHChangeNotify(uint e,uint f,IntPtr a,IntPtr b);}}'}
[VtxApp.Notice]::SHChangeNotify(0x08000000,0,[IntPtr]::Zero,[IntPtr]::Zero)
Write-Output "VTX main app installed in $InstallDirectory. Explorer preview registration and workers are unchanged."
