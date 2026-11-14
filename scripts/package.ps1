<#
.SYNOPSIS
    Builds the release exe, then the portable zip and installer, into dist\.

.DESCRIPTION
    Used by the release workflow and for local builds. Needs Inno Setup 6
    (winget install JRSoftware.InnoSetup).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\package.ps1
#>
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Find-InnoCompiler {
    $onPath = Get-Command iscc -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $candidates = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    )
    $found = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $found) { throw 'Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup' }
    $found
}

# Windows PowerShell turns a native tool's stderr (cargo's progress lines) into errors when output
# is redirected, so judge native tools by exit code only.
function Invoke-Native([string]$Name, [scriptblock]$Command) {
    $ErrorActionPreference = 'Continue'
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Name failed with exit code $LASTEXITCODE" }
}

$iscc = Find-InnoCompiler

Invoke-Native 'cargo build' { cargo build --release --locked }

$version = (Invoke-Native 'cargo metadata' { cargo metadata --no-deps --format-version 1 } | ConvertFrom-Json).packages |
    Where-Object name -eq 'nijimi' | Select-Object -ExpandProperty version
$tag = "v$version"

if (Test-Path dist) { Remove-Item dist -Recurse -Force }
$dist = (New-Item -ItemType Directory -Path dist).FullName

# Entries are named by hand: Windows PowerShell's zip helpers write `\` separators.
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::Open("$dist\nijimi-$tag-portable-x64.zip", 'Create')
try {
    foreach ($file in 'target/release/nijimi.exe', 'LICENSE') {
        $path = (Resolve-Path $file).Path
        [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $zip, $path, "nijimi/$(Split-Path $path -Leaf)", 'Optimal')
    }
} finally {
    $zip.Dispose()
}

Invoke-Native 'Inno Setup' { & $iscc /Q "/DAppVersion=$version" installer/nijimi.iss }

Get-ChildItem dist -File | Select-Object Name, @{ Name = 'Size (KB)'; Expression = { [math]::Round($_.Length / 1KB) } } |
    Format-Table -AutoSize
