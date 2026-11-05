[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
# Keep in sync with AGENTS.md section 4: the repo-wide limit is 1000 lines.
# Only files genuinely over that limit belong here; files between 800 and 1000
# are compliant and must not be grandfathered.
$exceptions = [System.Collections.Generic.HashSet[string]]::new([string[]]@(
    'crates/ark-agent/src/service/reactor.rs'
))
$violations = [System.Collections.Generic.List[string]]::new()

Get-ChildItem -Path (Join-Path $root 'crates'), (Join-Path $root 'apps') -Recurse -File -Filter '*.rs' |
    ForEach-Object {
        $relative = $_.FullName.Substring($root.Length + 1).Replace('\', '/')
        $lineCount = (Get-Content -LiteralPath $_.FullName).Count
        if ($lineCount -gt 1000 -and -not $exceptions.Contains($relative)) {
            $violations.Add("[max-lines] $relative has $lineCount lines (limit: 1000)")
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
