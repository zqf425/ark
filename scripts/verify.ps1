[CmdletBinding()]
param(
    [ValidateSet('core', 'web', 'contract', 'desktop', 'all')]
    [string]$Scope = 'all'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Invoke-CheckedNative {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments = @(),
        [switch]$CaptureOutput
    )

    if ($CaptureOutput) {
        $output = & $FilePath @Arguments
    } else {
        & $FilePath @Arguments
    }
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "$FilePath exited with code $exitCode"
    }
    if ($CaptureOutput) {
        return $output
    }
}

function Enable-GnuToolchain {
    if (-not (Get-Command dlltool.exe -ErrorAction SilentlyContinue) -and $env:ARK_MINGW_BIN) {
        $env:Path = "$env:ARK_MINGW_BIN;$env:Path"
    }
    $rustcVersion = Invoke-CheckedNative -FilePath rustc -Arguments @('-vV') -CaptureOutput
    $hostTarget = $rustcVersion | Select-String '^host:' | ForEach-Object { $_.ToString().Split(':')[1].Trim() }
    if ($hostTarget -like '*-gnu' -and -not (Get-Command dlltool.exe -ErrorAction SilentlyContinue)) {
        throw 'GNU Rust needs dlltool.exe. Add its directory to PATH or set ARK_MINGW_BIN before running verification.'
    }
}

function Invoke-CoreChecks {
    Enable-GnuToolchain
    Invoke-CheckedNative cargo @('fmt', '--all', '--', '--check')
    Invoke-CheckedNative cargo @('check', '--workspace', '--exclude', 'ark-desktop')
    Invoke-CheckedNative cargo @('test', '--workspace', '--exclude', 'ark-desktop')
    & "$PSScriptRoot/check-architecture.ps1"
}

function Invoke-WebChecks {
    Invoke-CheckedNative npm @('--prefix', 'web', 'test')
    Invoke-CheckedNative npm @('--prefix', 'web', 'run', 'check:rules')
    Invoke-CheckedNative npm @('--prefix', 'web', 'run', 'build')
}

function Invoke-ContractChecks {
    Enable-GnuToolchain
    Invoke-CheckedNative cargo @('test', '-p', 'ark-contract', '--lib', 'ts_export')
    Invoke-CheckedNative git @('diff', '--exit-code', 'web/src/generated')
}

function Invoke-DesktopChecks {
    Invoke-CheckedNative node @('--test', 'apps/ark-desktop/scripts/desktop-layout.test.mjs')
    Invoke-CheckedNative npm @('--prefix', 'apps/ark-desktop', 'run', 'build:web')
}

switch ($Scope) {
    'core' { Invoke-CoreChecks }
    'web' { Invoke-WebChecks }
    'contract' { Invoke-ContractChecks }
    'desktop' { Invoke-DesktopChecks }
    'all' {
        Invoke-CoreChecks
        Invoke-ContractChecks
        Invoke-WebChecks
        Invoke-DesktopChecks
    }
}
