# Phase gate: format, lint, test (incl. database tests when .env exists), audit.
$ErrorActionPreference = 'Stop'
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$envFile = "$PSScriptRoot\.env"
if (Test-Path $envFile) {
    # Temporary secret source (Bitwarden later). Values are never printed.
    $line = Get-Content $envFile | Where-Object { $_ -like 'DATABASE_URL=*' } | Select-Object -First 1
    if ($line) { $env:PRSWT_TEST_DATABASE_URL = ($line.Substring(13)) -replace '/prswt$', '/prswt_test' }
}
Push-Location "$PSScriptRoot\engine"
try {
    cargo fmt --all -- --check;                              if ($LASTEXITCODE) { throw 'fmt failed' }
    cargo clippy --workspace --all-targets -- -D warnings;   if ($LASTEXITCODE) { throw 'clippy failed' }
    cargo nextest run --workspace;                           if ($LASTEXITCODE) { throw 'tests failed' }
    $env:RUSTDOCFLAGS = '-D warnings'
    cargo doc --workspace --no-deps -q;                      if ($LASTEXITCODE) { throw 'rustdoc failed' }
    cargo test --workspace --doc;                            if ($LASTEXITCODE) { throw 'doctests failed' }
    cargo deny check --hide-inclusion-graph 2>&1 | Select-String 'error\[|ok$'; if ($LASTEXITCODE) { throw 'cargo deny failed' }
    cargo run -q -p xtask -- layers;                         if ($LASTEXITCODE) { throw 'layering failed' }
    Write-Host 'GATE PASSED' -ForegroundColor Green
} finally { Pop-Location }
