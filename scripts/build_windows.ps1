$ErrorActionPreference = "Stop"
$bundledModule = Join-Path $PSScriptRoot "..\src\versebridge\_bundled_credentials.py"
$apiBibleKey = if ($env:API_BIBLE_API_KEY) { $env:API_BIBLE_API_KEY } else { [Environment]::GetEnvironmentVariable("API_BIBLE_API_KEY", "User") }
$youVersionKey = if ($env:YOUVERSION_API_KEY) { $env:YOUVERSION_API_KEY } else { [Environment]::GetEnvironmentVariable("YOUVERSION_API_KEY", "User") }

if (-not $apiBibleKey -or -not $youVersionKey) {
    throw "Both API_BIBLE_API_KEY and YOUVERSION_API_KEY must be available to build the self-contained executable."
}

function ConvertTo-PythonString([string]$value) {
    return $value.Replace("\", "\\").Replace("'", "\'")
}

try {
    $module = "API_BIBLE_API_KEY = '$(ConvertTo-PythonString $apiBibleKey)'`nYOUVERSION_API_KEY = '$(ConvertTo-PythonString $youVersionKey)'`n"
    [System.IO.File]::WriteAllText($bundledModule, $module, [System.Text.UTF8Encoding]::new($false))
    uv sync --extra dev
    $generatedPath = Join-Path $PSScriptRoot "..\src\versebridge\presentation\generated"
    $pyinstallerArgs = @(
        "--onefile", "--name", "versebridge",
        "--paths", $generatedPath,
        "--hidden-import", "versebridge._bundled_credentials"
    )
    foreach ($binding in Get-ChildItem -LiteralPath $generatedPath -Filter "*_pb2.py") {
        $pyinstallerArgs += @("--hidden-import", $binding.BaseName)
    }
    $pyinstallerArgs += "src/versebridge/cli.py"
    uv run pyinstaller @pyinstallerArgs
}
finally {
    Remove-Item -LiteralPath $bundledModule -Force -ErrorAction SilentlyContinue
}
