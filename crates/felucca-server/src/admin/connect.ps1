param([string]$Server, [string]$Device, [string]$Token, [string]$Config, [string]$Language = 'en')
$ErrorActionPreference = 'Stop'
if ($Device -notmatch '^[a-zA-Z0-9_-][a-zA-Z0-9_.-]*$' -or !$Token -or !$Config -or $Language -notmatch '^(en|zh|ja|ko|fr|es|ru)$') { throw {{invalid}} }
$architecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$target = switch ($architecture) {
    'ARM64' { 'aarch64-pc-windows-msvc' }
    'AMD64' { 'x86_64-pc-windows-msvc' }
    default { throw {{unsupported}} }
}
$headers = @{ Authorization = "Bearer $Token"; 'Accept-Language' = $Language }
$url = "$Server/api/connect/$Device/$target"
try { $hash = (Invoke-WebRequest -UseBasicParsing -Uri "$url/sha256" -Headers $headers -TimeoutSec 30).Content.Trim() }
catch { throw {{unavailable}} }
if ($hash -notmatch '^[a-f0-9]{64}$') { throw {{checksum}} }
$base = if ($env:FELUCCA_DATA_DIR) { $env:FELUCCA_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'Felucca' }
$hasher = [Security.Cryptography.SHA256]::Create()
try { $serverHash = [BitConverter]::ToString($hasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($Server))).Replace('-', '').ToLowerInvariant() }
finally { $hasher.Dispose() }
$folder = Join-Path (Join-Path $base $serverHash) $Device
$binaryFolder = Join-Path (Join-Path $base 'bin') $hash
$binary = Join-Path $binaryFolder 'felucca-agent.exe'
$work = Join-Path $base ('.connect-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $folder, $binaryFolder, $work | Out-Null
try {
    if (!(Test-Path $binary) -or (Get-FileHash -Algorithm SHA256 $binary).Hash.ToLowerInvariant() -ne $hash) {
        $download = Join-Path $work 'felucca-agent.exe'
        try { Invoke-WebRequest -UseBasicParsing -Uri "$url/binary" -Headers $headers -OutFile $download -TimeoutSec 600 }
        catch { throw {{unavailable}} }
        if ((Get-FileHash -Algorithm SHA256 $download).Hash.ToLowerInvariant() -ne $hash) { throw {{checksum}} }
        Move-Item -Force $download $binary
    }
    $draft = Join-Path $work 'connection.json'
    [IO.File]::WriteAllBytes($draft, [Convert]::FromBase64String($Config))
    & $binary --config $draft --check --lang $Language
    if ($LASTEXITCODE -ne 0) { throw {{invalid}} }
    $file = Join-Path $folder 'connection.json'
    Move-Item -Force $draft $file
    Write-Host {{starting}}
    & $binary --config $file --exclusive --lang $Language
    if ($LASTEXITCODE -ne 0) { throw "Felucca: $LASTEXITCODE" }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
