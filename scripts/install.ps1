# Install the boxquell command on Windows.
#
#   irm https://github.com/Boxquell/boxquell/releases/latest/download/install.ps1 | iex
#
# It downloads the latest release, checks it against the release's SHA256SUMS
# and puts boxquell.exe into %LOCALAPPDATA%\Programs\Boxquell, which it adds to
# your PATH. Nothing needs administrator rights. Run it again to update.
#
# Parameters (when run from a file):
#   -Dir <path>     install into this folder instead
#   -Uninstall      remove the command again (sandboxes and settings stay)
# Environment:
#   BOXQUELL_INSTALL_DIR   same as -Dir
#   BOXQUELL_RELEASE_URL   folder to download from; default is the latest release
param(
    [string]$Dir = $env:BOXQUELL_INSTALL_DIR,
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
# Windows PowerShell 5 downloads slowly with a progress bar, and needs TLS 1.2 named.
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$releaseUrl = if ($env:BOXQUELL_RELEASE_URL) { $env:BOXQUELL_RELEASE_URL } else { 'https://github.com/Boxquell/boxquell/releases/latest/download' }
if (-not $Dir) { $Dir = Join-Path $env:LOCALAPPDATA 'Programs\Boxquell' }

function Step($text) { Write-Host '> ' -ForegroundColor Cyan -NoNewline; Write-Host $text }
function Ok($text) { Write-Host 'v ' -ForegroundColor Green -NoNewline; Write-Host $text }
function Note($text) { Write-Host '! ' -ForegroundColor Yellow -NoNewline; Write-Host $text }
function Fail($text) { Write-Host "x $text" -ForegroundColor Red; exit 1 }

Write-Host ''
Write-Host '      ^' -ForegroundColor Cyan
Write-Host '     /#\      ' -ForegroundColor Cyan -NoNewline; Write-Host 'Boxquell'
Write-Host '   [=====]    ' -NoNewline; Write-Host 'MicroVM sandboxes for coding agents' -ForegroundColor DarkGray
Write-Host '   [_____]'
Write-Host ''

# The folder as one entry of the user's PATH.
function Set-UserPath([string]$folder, [bool]$present) {
    $entries = @(([Environment]::GetEnvironmentVariable('Path', 'User') -split ';') | Where-Object { $_ -and ($_.TrimEnd('\') -ne $folder.TrimEnd('\')) })
    if ($present) { $entries += $folder }
    [Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
}

$exe = Join-Path $Dir 'boxquell.exe'

if ($Uninstall) {
    if (Test-Path $exe) {
        Remove-Item $exe -Force
        Set-UserPath $Dir $false
        Ok "Removed $exe"
        Write-Host '  Your sandboxes and settings were left alone.' -ForegroundColor DarkGray
    } else {
        Note "There is no boxquell in $Dir."
    }
    exit 0
}

if (-not [Environment]::Is64BitOperatingSystem -or $env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    Fail 'Boxquell for Windows is built for 64-bit Intel and AMD processors only.'
}
$target = 'x86_64-pc-windows-msvc'

$work = Join-Path ([IO.Path]::GetTempPath()) ("boxquell-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    Step 'Looking up the latest release for Windows'
    $sums = Join-Path $work 'SHA256SUMS'
    try { Invoke-WebRequest "$releaseUrl/SHA256SUMS" -OutFile $sums -UseBasicParsing } catch { Fail "Could not download the list of release files from $releaseUrl" }

    # The checksum list names every file of the release; pick the one for this computer.
    $line = Get-Content $sums | Where-Object { $_ -match "^([0-9a-f]{64})  (boxquell-v?[0-9][^ ]*-$([regex]::Escape($target))\.zip)$" } | Select-Object -First 1
    if (-not $line) { Fail "The latest release has no build for $target" }
    $null = $line -match "^([0-9a-f]{64})  (.+)$"
    $expected = $Matches[1]
    $asset = $Matches[2]
    $version = ($asset -replace '^boxquell-v?', '') -replace "-$([regex]::Escape($target))\.zip$", ''

    $previous = if (Test-Path $exe) { ((& $exe --version) -split ' ')[1] } else { $null }

    Step "Downloading boxquell $version"
    $archive = Join-Path $work $asset
    try { Invoke-WebRequest "$releaseUrl/$asset" -OutFile $archive -UseBasicParsing } catch { Fail "Could not download $asset" }
    $actual = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { Fail "The download does not match its checksum (expected $expected, got $actual). Nothing was installed." }
    Ok 'Checksum matches'

    Expand-Archive $archive -DestinationPath (Join-Path $work 'unpacked')
    $binary = Get-ChildItem (Join-Path $work 'unpacked') -Recurse -Filter 'boxquell.exe' | Select-Object -First 1
    if (-not $binary) { Fail 'The release archive contains no boxquell.exe' }

    New-Item -ItemType Directory -Path $Dir -Force | Out-Null
    try {
        Copy-Item $binary.FullName $exe -Force
    } catch {
        Fail "Could not replace $exe. If boxquell is running, stop it with 'boxquell daemon --stop' and try again."
    }
    Set-UserPath $Dir $true

    if ($previous -and $previous -ne $version) { Ok "Updated boxquell $previous -> $version in $Dir" } else { Ok "Installed boxquell $version to $Dir" }

    Write-Host ''
    Write-Host 'What boxquell needs'
    $missing = 0
    function Check([bool]$present, [string]$name, [string]$how) {
        if ($present) { Ok $name } else { Note "$name is missing  ($how)"; $script:missing++ }
    }
    Check ([bool](Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue)) 'QEMU (runs the sandboxes)' 'winget install SoftwareFreedomConservancy.QEMU, and add it to PATH'
    Check ([bool](Get-Command ssh -ErrorAction SilentlyContinue)) 'OpenSSH client' 'Settings > Optional features > OpenSSH Client'
    Note 'Boxquell on Windows has not been tested yet. It uses QEMU with the Windows Hypervisor Platform, which has to be enabled under "Turn Windows features on or off".'

    Write-Host ''
    Write-Host 'Next'
    Write-Host '  Open a new terminal so the changed PATH applies, then:' -ForegroundColor DarkGray
    if ($missing -gt 0) { Write-Host '  Install what is marked above first.' }
    Write-Host '  boxquell doctor' -ForegroundColor Cyan -NoNewline; Write-Host '              checks that everything is in place' -ForegroundColor DarkGray
    Write-Host '  boxquell new mybox --backend qemu' -ForegroundColor Cyan -NoNewline; Write-Host '   a sandbox; the system is downloaded on first use' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '  Docs: https://github.com/Boxquell/boxquell' -ForegroundColor DarkGray
    Write-Host ''
} finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
