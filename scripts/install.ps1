# Installs the `wisp` command on Windows from the GitHub release, checks its
# SHA-256, puts wisp.exe in $env:WISP_INSTALL_DIR (default
# %LOCALAPPDATA%\wisp\bin) and adds that to the user PATH.
# $env:WISP_VERSION = "v0.1.0" picks a version (default: latest).
# Meant to be served at https://wispweb.dev/install.ps1 (not yet).
$ErrorActionPreference = "Stop"

$repo = "wyziedevs/wisp"
$dir = if ($env:WISP_INSTALL_DIR) { $env:WISP_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "wisp\bin" }
$version = if ($env:WISP_VERSION) { $env:WISP_VERSION } else { "latest" }
# Windows on ARM runs the x86_64 build in its emulation.
if ($env:PROCESSOR_ARCHITECTURE -notin "AMD64", "ARM64") {
    throw "wisp: no prebuilt binary for $env:PROCESSOR_ARCHITECTURE; run: cargo install wisp-web"
}
$name = "wisp-x86_64-pc-windows-msvc"
$base = if ($version -eq "latest") { "https://github.com/$repo/releases/latest/download" } else { "https://github.com/$repo/releases/download/$version" }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("wisp-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp "$name.zip"
    Invoke-WebRequest -UseBasicParsing "$base/$name.zip" -OutFile $zip
    Invoke-WebRequest -UseBasicParsing "$base/$name.zip.sha256" -OutFile "$zip.sha256"
    $want = ((Get-Content "$zip.sha256" -Raw).Trim() -split "\s+")[0].ToLower()
    $got = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    if ($want -ne $got) { throw "wisp: checksum mismatch for $name.zip (want $want, got $got)" }
    Expand-Archive -Path $zip -DestinationPath $tmp
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item (Join-Path $tmp "$name\wisp.exe") (Join-Path $dir "wisp.exe") -Force
} finally {
    Remove-Item -Recurse -Force $tmp
}
$path = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not ($path -split ";" | Where-Object { $_ -eq $dir })) {
    [Environment]::SetEnvironmentVariable("Path", "$path;$dir", "User")
    Write-Host "Added $dir to your PATH (new terminals)."
}
Write-Host "wisp installed to $dir\wisp.exe"
