$ErrorActionPreference = "Stop"
$bundledModule = Join-Path $PSScriptRoot "..\src\versevision\_bundled_credentials.py"
# Every provider key the app can use. A key may come from the environment, the Windows user environment,
# or keys.env (NAME=value lines) in %APPDATA%\versevision. Missing keys are skipped with a warning;
# at least one key is required.
$names = @("API_BIBLE_API_KEY", "YOUVERSION_API_KEY", "ESV_API_KEY")
$keysFile = Join-Path $env:APPDATA "versevision\keys.env"
$fromFile = @{}
if (Test-Path -LiteralPath $keysFile) {
    foreach ($line in Get-Content -LiteralPath $keysFile) {
        if ($line -match '^\s*([A-Z0-9_]+)\s*=\s*(.+?)\s*$' -and -not $line.TrimStart().StartsWith("#")) { $fromFile[$Matches[1]] = $Matches[2].Trim('"', "'") }
    }
}
$values = [ordered]@{}
foreach ($name in $names) {
    $value = [Environment]::GetEnvironmentVariable($name, "Process")
    if (-not $value) { $value = [Environment]::GetEnvironmentVariable($name, "User") }
    if (-not $value -and $fromFile.ContainsKey($name)) { $value = $fromFile[$name] }
    if ($value) { $values[$name] = $value } else { Write-Warning "$name was not found; the executable will not include it." }
}
if ($values.Count -eq 0) { throw "No provider keys were found. Set API_BIBLE_API_KEY, YOUVERSION_API_KEY and/or ESV_API_KEY." }

function ConvertTo-PythonString([string]$value) {
    return $value.Replace("\", "\\").Replace("'", "\'")
}

try {
    $module = ($values.GetEnumerator() | ForEach-Object { "$($_.Key) = '$(ConvertTo-PythonString $_.Value)'" }) -join "`n"
    [System.IO.File]::WriteAllText($bundledModule, $module + "`n", [System.Text.UTF8Encoding]::new($false))
    uv sync --extra dev
    $generatedPath = Join-Path $PSScriptRoot "..\src\versevision\presentation\generated"
    $pyinstallerArgs = @(
        "--onefile", "--name", "versevision",
        "--paths", $generatedPath,
        "--add-data", "src\versevision\web\index.html;versevision\web",
        "--hidden-import", "versevision._bundled_credentials"
    )
    foreach ($binding in Get-ChildItem -LiteralPath $generatedPath -Filter "*_pb2.py") {
        $pyinstallerArgs += @("--hidden-import", $binding.BaseName)
    }
    $pyinstallerArgs += "src/versevision/cli.py"
    uv run pyinstaller @pyinstallerArgs
}
finally {
    Remove-Item -LiteralPath $bundledModule -Force -ErrorAction SilentlyContinue
}
