# Install the boxquell command on Windows.
#
#   irm https://github.com/Boxquell/boxquell/releases/latest/download/install.ps1 | iex
#
# It downloads the latest release, checks it against the release's SHA256SUMS
# and puts boxquell.exe into %LOCALAPPDATA%\Programs\Boxquell, which it adds to
# your PATH. The desktop app comes with it, with an entry in the Start menu.
# Nothing needs administrator rights. Run it again to update.
#
# Parameters (when run from a file):
#   -Dir <path>     install into this folder instead
#   -NoDesktop      install the command only, without the desktop app
#   -Deps           install what boxquell needs (QEMU, OpenSSH, the Windows
#                   Hypervisor Platform) without asking; -NoDeps never does
#   -Uninstall      remove the command and the app again (sandboxes and settings stay)
# Environment:
#   BOXQUELL_INSTALL_DIR   same as -Dir
#   BOXQUELL_RELEASE_URL   folder to download from; default is the latest release
param(
    [string]$Dir = $env:BOXQUELL_INSTALL_DIR,
    [switch]$NoDesktop,
    [switch]$Deps,
    [switch]$NoDeps,
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
# Where the desktop app's installer puts it, with its own uninstaller.
$appUninstaller = Join-Path $env:LOCALAPPDATA 'Boxquell\uninstall.exe'

if ($Uninstall) {
    if (Test-Path $exe) {
        Remove-Item $exe -Force
        Set-UserPath $Dir $false
        Ok "Removed $exe"
        Write-Host '  Your sandboxes and settings were left alone.' -ForegroundColor DarkGray
    } else {
        Note "There is no boxquell in $Dir."
    }
    if (Test-Path $appUninstaller) {
        Start-Process -FilePath $appUninstaller -ArgumentList '/S' -Wait
        Ok 'Removed the desktop app and its Start menu entry'
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

    # The desktop app, when this release has one. The command works without it.
    $appInstalled = $false
    if (-not $NoDesktop -and -not $env:BOXQUELL_NO_DESKTOP) {
        $appLine = Get-Content $sums | Where-Object { $_ -match "^([0-9a-f]{64})  (boxquell-desktop-[^ ]*-$([regex]::Escape($target))-setup\.exe)$" } | Select-Object -First 1
        if (-not $appLine) {
            Note 'This release has no desktop app for Windows; the command works without it.'
        } else {
            $null = $appLine -match "^([0-9a-f]{64})  (.+)$"
            $appSum = $Matches[1]
            $appAsset = $Matches[2]
            Step 'Downloading the desktop app'
            $setup = Join-Path $work $appAsset
            $downloaded = $true
            try { Invoke-WebRequest "$releaseUrl/$appAsset" -OutFile $setup -UseBasicParsing } catch { $downloaded = $false }
            if (-not $downloaded) {
                Note 'Could not download the desktop app; the command works without it.'
            } else {
                $appActual = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($appActual -ne $appSum) { Fail 'The desktop app does not match its checksum. It was not installed.' }
                # Its installer works for this user only and without questions.
                $run = Start-Process -FilePath $setup -ArgumentList '/S' -Wait -PassThru
                if ($run.ExitCode -eq 0) {
                    Ok 'Installed the desktop app; Boxquell is in your Start menu'
                    $appInstalled = $true
                } else {
                    Note "The desktop app's installer ended with code $($run.ExitCode); the command works without it."
                }
            }
        }
    }

    # QEMU's installer does not put it on the PATH; look in its folder too.
    $qemuDir = Join-Path $env:ProgramFiles 'qemu'
    function Test-Qemu { [bool](Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue) -or (Test-Path (Join-Path $qemuDir 'qemu-system-x86_64.exe')) }
    function Test-Ssh { [bool](Get-Command ssh -ErrorAction SilentlyContinue) }
    # Whether the Windows Hypervisor Platform is on; $null when only an administrator can tell.
    function Test-Whpx {
        try { (Get-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -ErrorAction Stop).State -eq 'Enabled' } catch { $null }
    }
    function Show-Needs {
        $script:missing = 0
        function Check([bool]$present, [string]$name, [string]$how) {
            if ($present) { Ok $name } else { Note "$name is missing  ($how)"; $script:missing++ }
        }
        Check (Test-Qemu) 'QEMU (runs the sandboxes)' 'winget install SoftwareFreedomConservancy.QEMU'
        Check (Test-Ssh) 'OpenSSH client' 'Settings > Optional features > OpenSSH Client'
        $whpx = Test-Whpx
        if ($null -eq $whpx) {
            Note 'Windows Hypervisor Platform: could not be checked without administrator rights. It has to be on under "Turn Windows features on or off".'
        } else {
            Check $whpx 'Windows Hypervisor Platform' 'Turn Windows features on or off > Windows Hypervisor Platform, then restart'
        }
    }

    Write-Host ''
    Write-Host 'What boxquell needs'
    $missing = 0
    Show-Needs
    Note 'Boxquell on Windows has not been tested yet.'

    if ($missing -gt 0 -and -not $NoDeps) {
        $isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        # What can be done as this user, and what only an administrator can do.
        $userSteps = @()
        $adminSteps = @()
        if (-not (Test-Qemu)) { $userSteps += 'winget install --id SoftwareFreedomConservancy.QEMU -e --accept-source-agreements --accept-package-agreements' }
        if (-not (Test-Ssh)) { $adminSteps += 'Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0' }
        if ((Test-Whpx) -eq $false) { $adminSteps += 'Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -All -NoRestart' }
        $steps = if ($isAdmin) { $userSteps + $adminSteps } else { $userSteps }
        if ($steps.Count -gt 0) {
            Write-Host ''
            Write-Host 'These commands install what is missing:'
            $steps | ForEach-Object { Write-Host "    $_" -ForegroundColor Cyan }
            $run = [bool]$Deps
            # Ask only when somebody can answer.
            if (-not $Deps -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected) {
                try { $run = (Read-Host '? Run them now? [y/N]') -match '^(y|yes|j|ja)$' } catch { $run = $false }
            }
            if ($run) {
                foreach ($step in $steps) {
                    Step $step
                    try { Invoke-Expression $step | Out-Null } catch { Note 'That did not work; run it yourself to see why.' }
                }
                Write-Host ''
                Write-Host 'What boxquell needs'
                Show-Needs
                if ($steps -match 'HypervisorPlatform') { Note 'Restart Windows for the Hypervisor Platform to apply.' }
            }
        }
        if (-not $isAdmin -and $adminSteps.Count -gt 0) {
            Write-Host ''
            Write-Host 'These need a PowerShell started as administrator:'
            $adminSteps | ForEach-Object { Write-Host "    $_" -ForegroundColor Cyan }
        }
    }
    # QEMU is found by boxquell on the PATH.
    if ((Test-Path (Join-Path $qemuDir 'qemu-system-x86_64.exe')) -and -not (Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue)) {
        Set-UserPath $qemuDir $true
        Ok "Added $qemuDir to your PATH"
    }

    Write-Host ''
    Write-Host 'Next'
    Write-Host '  Open a new terminal so the changed PATH applies, then:' -ForegroundColor DarkGray
    if ($missing -gt 0) { Write-Host '  Install what is marked above first.' }
    if ($appInstalled) { Write-Host '  Open Boxquell from the Start menu, or use the command:' }
    Write-Host '  boxquell doctor' -ForegroundColor Cyan -NoNewline; Write-Host '              checks that everything is in place' -ForegroundColor DarkGray
    Write-Host '  boxquell new mybox --backend qemu' -ForegroundColor Cyan -NoNewline; Write-Host '   a sandbox; the system is downloaded on first use' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '  Docs: https://github.com/Boxquell/boxquell' -ForegroundColor DarkGray
    Write-Host ''
} finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
