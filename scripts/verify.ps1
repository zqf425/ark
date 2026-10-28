[CmdletBinding()]
param(
    [ValidateSet('core', 'web', 'contract', 'desktop', 'all')]
    [string]$Scope = 'all'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Enable-GnuToolchain {
    if (-not (Get-Command dlltool.exe -ErrorAction SilentlyContinue) -and $env:ARK_MINGW_BIN) {
        $env:Path = "$env:ARK_MINGW_BIN;$env:Path"
    }
    $hostTarget = rustc -vV | Select-String '^host:' | ForEach-Object { $_.ToString().Split(':')[1].Trim() }
    if ($hostTarget -like '*-gnu' -and -not (Get-Command dlltool.exe -ErrorAction SilentlyContinue)) {
        throw 'GNU Rust needs dlltool.exe. Add its directory to PATH or set ARK_MINGW_BIN before running verification.'
    }
}

function Invoke-CoreChecks {
    Enable-GnuToolchain
    & cargo fmt --all -- --check
    & cargo check --workspace --exclude ark-desktop
    & cargo test --workspace --exclude ark-desktop
    & "$PSScriptRoot/check-architecture.ps1"
}

function Invoke-WebChecks {
    & npm --prefix web test
    & npm --prefix web run check:rules
    & npm --prefix web run build
}

function Invoke-ContractChecks {
    Enable-GnuToolchain
    & cargo test -p ark-contract --lib ts_export
    & git diff --exit-code web/src/generated
}

function Invoke-DesktopChecks {
    & node --test apps/ark-desktop/scripts/desktop-layout.test.mjs
    & npm --prefix apps/ark-desktop run build:web
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
