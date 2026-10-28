[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exceptions = [System.Collections.Generic.HashSet[string]]::new([string[]]@(
    'crates/ark-contract/src/event.rs',
    'crates/ark-delegation/src/service/runtime.rs',
    'crates/ark-delegation/src/service/runtime/tests.rs',
    'crates/ark-provider/src/service/store.rs',
    'crates/ark-agent/src/service/reactor.rs',
    'apps/arkd/src/wiring.rs'
))
$violations = [System.Collections.Generic.List[string]]::new()

Get-ChildItem -Path (Join-Path $root 'crates'), (Join-Path $root 'apps') -Recurse -File -Filter '*.rs' |
    ForEach-Object {
        $relative = $_.FullName.Substring($root.Length + 1).Replace('\', '/')
        $lineCount = (Get-Content -LiteralPath $_.FullName).Count
        if ($lineCount -gt 800 -and -not $exceptions.Contains($relative)) {
            $violations.Add("[max-lines] $relative has $lineCount lines (limit: 800)")
        }
        if ((Get-Content -Raw -LiteralPath $_.FullName) -match 'pub use \*;') {
            $violations.Add("[no-glob-reexport] $relative exports a glob")
        }
    }

if ($violations.Count -gt 0) {
    $violations | ForEach-Object { Write-Error $_ }
    exit 1
}

Write-Host 'Architecture checks passed.'
