# roadcase on Windows, end to end: packages scripts/e2e-app (a tiny application
# linking roadcase-update, with a helper program and a resource folder) as
# 1.0.0 and 2.0.0 with `roadcase release windows`, unsigned, through the
# example installer script (examples/helper-app/packaging/windows/installer.nsi),
# and checks each release: the installer, the feed folder (the installer under
# its versioned name, latest.json and its signature), `roadcase verify`.
#
# Then it installs 1.0.0 silently for this person, checks the files, the
# uninstall entry, the URL scheme and the document type in the registry,
# serves the feed on 127.0.0.1, and lets 1.0.0 update itself: with 1.0.0 in
# the feed it must say it is up to date; with 2.0.0 it must download the
# installer, start it with /S /UPDATE /RELAUNCH=relaunched, and be started
# again as 2.0.0. Last, it uninstalls silently and checks that the files and
# the registry entries are gone.
#
# Run on Windows, natively (no cross-building), from anywhere:
#
#   pwsh scripts/windows-e2e.ps1
#
# Needs cargo (MSVC), NSIS (makensis on PATH, in its default folder, or named
# by $env:MAKENSIS; `choco install nsis -y`, which the script runs itself in
# CI when NSIS is missing) and python (to serve the feed).
# Builds into $env:CARGO_TARGET_DIR, or the repository's target\. $env:PORT is
# the feed's port (8767). $env:KEEP = 1 keeps the temporary folder.
#
# It installs into %LOCALAPPDATA%\Roadcase Fixture and writes under HKCU, as
# any installed application does, and removes both again, also when a check
# fails.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$fixture = Join-Path $root 'scripts\e2e-app'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$port = if ($env:PORT) { $env:PORT } else { '8767' }
$work = Join-Path ([IO.Path]::GetTempPath()) ('roadcase-e2e-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null

$name = 'Roadcase Fixture'
$id = 'com.example.roadcase-fixture'
$install = Join-Path $env:LOCALAPPDATA $name
$exe = Join-Path $install 'fixture.exe'
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$name"
$schemeKey = 'HKCU:\Software\Classes\roadcase-fixture'
$extensionKey = 'HKCU:\Software\Classes\.rcfixture'
$progId = "$id.FixtureDocument"
$shortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\$name.lnk"
$server = $null

function Fail([string]$message) { throw "FAIL: $message" }
function Pass([string]$message) { Write-Host "ok: $message" }

# Runs a program and answers with its exit code and everything it wrote.
# What a native program writes to standard error must not stop the script.
function Invoke-Native([string]$file, [string[]]$arguments) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $lines = & $file @arguments 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previous
    }
    [pscustomobject]@{ Code = $code; Output = (@($lines) -join "`n") }
}

# Runs a program that must succeed, and answers with what it wrote.
function Invoke-Checked([string]$what, [string]$file, [string[]]$arguments) {
    $result = Invoke-Native $file $arguments
    if ($result.Code -ne 0) {
        Write-Host $result.Output
        Fail "$what (exit code $($result.Code))"
    }
    $result.Output
}

function Wait-Until([scriptblock]$condition, [int]$seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        if (& $condition) { return $true }
        Start-Sleep -Milliseconds 200
    }
    [bool](& $condition)
}

function Get-Default([string]$key) {
    if (-not (Test-Path -LiteralPath $key)) { return $null }
    (Get-Item -LiteralPath $key).GetValue('')
}

# Removes the installed fixture: before the run, when an earlier one left it,
# and after a failed one.
function Remove-Installed {
    $uninstaller = Join-Path $install 'uninstall.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait
        Wait-Until { -not (Test-Path -LiteralPath $exe) } 30 | Out-Null
    }
}

try {
    # makensis: on PATH, named, or where NSIS installs itself.
    if (-not $env:MAKENSIS -and -not (Get-Command makensis -ErrorAction SilentlyContinue)) {
        $default = Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'
        # In CI, on a runner without NSIS, it is installed for the run.
        if (-not (Test-Path -LiteralPath $default) -and $env:CI -and (Get-Command choco -ErrorAction SilentlyContinue)) {
            Invoke-Checked 'installing NSIS' 'choco' @('install', 'nsis', '-y', '--no-progress') | Out-Null
        }
        if (-not (Test-Path -LiteralPath $default)) {
            Fail 'makensis is not on PATH: install NSIS (choco install nsis -y), or name it in $env:MAKENSIS'
        }
        $env:MAKENSIS = $default
    }
    $python = Get-Command python -ErrorAction SilentlyContinue
    if (-not $python) { Fail 'python is not on PATH: it serves the feed' }
    Remove-Installed

    $env:CARGO_TARGET_DIR = $target
    Push-Location $root
    try {
        Invoke-Checked 'building roadcase' 'cargo' @('build', '--quiet', '-p', 'roadcase') | Out-Null
    }
    finally {
        Pop-Location
    }
    $roadcase = Join-Path $target 'debug\roadcase.exe'

    $keys = Join-Path $work 'keys'
    Invoke-Checked 'roadcase keygen' $roadcase @('keygen', $keys) | Out-Null
    $publicKey = Join-Path $keys 'update.pub'
    $env:FIXTURE_DIR = $fixture
    $env:FIXTURE_TARGET = Join-Path $target 'e2e-app'
    $env:FIXTURE_FEED = "http://127.0.0.1:$port/"
    $env:FIXTURE_PUBLIC_KEY = (Get-Content -LiteralPath $publicKey -Raw).Trim()
    $env:ROADCASE_UPDATE_SECRET_KEY = Get-Content -LiteralPath (Join-Path $keys 'update.key') -Raw
    # Unsigned: the fixture's config has no [windows.sign].

    # Packages one version and checks what every release must hold.
    function New-Release([string]$version) {
        $out = Join-Path $work "out-$version"
        $feed = Join-Path $work "feed-$version"
        $env:FIXTURE_VERSION = $version
        $env:FIXTURE_FEED_OUT = $feed
        $env:FIXTURE_NOTES = "Release $version."
        $said = Invoke-Checked "roadcase release windows ($version)" $roadcase @(
            'release', 'windows', '--config', (Join-Path $fixture 'roadcase.toml'), '--out', $out)
        $setup = Join-Path $out "$name-Setup.exe"
        if (-not (Test-Path -LiteralPath $setup)) { Fail "${version}: no installer at $setup`n$said" }
        $staged = Join-Path $out "$name.windows"
        foreach ($file in @('fixture.exe', 'fixture-helper.exe', 'resources\docs\guide.md')) {
            if (-not (Test-Path -LiteralPath (Join-Path $staged $file))) { Fail "${version}: $file is not staged" }
        }
        $named = "$name-$version-x64-setup.exe"
        foreach ($file in @($named, 'latest.json', 'latest.json.minisig')) {
            if (-not (Test-Path -LiteralPath (Join-Path $feed $file))) { Fail "${version}: the feed lacks $file" }
        }
        $verified = Invoke-Checked "roadcase verify ($version)" $roadcase @('verify', '--key', $publicKey, $feed)
        if ($verified -notmatch [regex]::Escape("windows-x86_64: $named matches")) {
            Fail "${version}: the feed's installer does not verify`n$verified"
        }
        $manifest = Get-Content -LiteralPath (Join-Path $feed 'latest.json') -Raw | ConvertFrom-Json
        if ($manifest.version -ne $version) { Fail "${version}: the feed says $($manifest.version)" }
        if ($manifest.notes -ne "Release $version.") { Fail "${version}: the feed lacks the notes" }
        if ($manifest.artifacts.'windows-x86_64'.kind -ne 'nsis') { Fail "${version}: the feed's installer is not an nsis artifact" }
        Pass "${version}: the installer is made, and the feed verifies"
        $setup
    }

    $setup1 = New-Release '1.0.0'
    New-Release '2.0.0' | Out-Null

    # A person installs 1.0.0, silently here.
    $installer = Start-Process -FilePath $setup1 -ArgumentList '/S' -Wait -PassThru
    if ($installer.ExitCode -ne 0) { Fail "the 1.0.0 installer left with $($installer.ExitCode)" }
    if (-not (Test-Path -LiteralPath $exe)) { Fail "1.0.0 is not installed in $install" }
    if ((Invoke-Checked 'fixture version' $exe @('version')).Trim() -ne '1.0.0') { Fail 'the installed application is not 1.0.0' }
    if ((Invoke-Checked 'fixture helper' $exe @('helper')).Trim() -ne 'helper 1.0.0') { Fail 'the installed helper does not run' }
    foreach ($file in @('fixture-helper.exe', 'resources\docs\guide.md', 'uninstall.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $install $file))) { Fail "1.0.0 did not install $file" }
    }
    if (-not (Test-Path -LiteralPath $shortcut)) { Fail 'the Start menu has no shortcut' }
    Pass '1.0.0 installs silently, for this person, with its helper, resources and shortcut'

    $entry = Get-ItemProperty -LiteralPath $uninstallKey
    if ($entry.DisplayVersion -ne '1.0.0') { Fail "the uninstall entry says $($entry.DisplayVersion)" }
    if ($entry.InstallLocation -ne $install) { Fail "the uninstall entry names $($entry.InstallLocation)" }
    if ($entry.Publisher -ne 'Example Inc.') { Fail "the uninstall entry's publisher is $($entry.Publisher)" }
    $command = "`"$exe`" `"%1`""
    if ((Get-Default "$schemeKey\shell\open\command") -ne $command) {
        Fail "the URL scheme opens with $(Get-Default "$schemeKey\shell\open\command")"
    }
    if ($null -eq (Get-Item -LiteralPath $schemeKey).GetValue('URL Protocol')) { Fail 'the URL scheme is not a URL protocol' }
    if ((Get-Default $extensionKey) -ne $progId) { Fail ".rcfixture opens with $(Get-Default $extensionKey)" }
    if ((Get-Default "HKCU:\Software\Classes\$progId\shell\open\command") -ne $command) {
        Fail 'the document type has no open command'
    }
    Pass 'the uninstall entry, the URL scheme and the document type are in the registry'

    # The feed, served as a host would serve it.
    $served = Join-Path $work 'served'
    New-Item -ItemType Directory -Path $served | Out-Null
    Copy-Item -Path (Join-Path $work 'feed-1.0.0\*') -Destination $served
    $server = Start-Process -FilePath $python.Source -WorkingDirectory $served -WindowStyle Hidden -PassThru `
        -ArgumentList @('-m', 'http.server', $port, '--bind', '127.0.0.1')
    $up = Wait-Until {
        try { (Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$port/latest.json").StatusCode -eq 200 } catch { $false }
    } 15
    if (-not $up) { Fail "the feed is not served on port $port" }

    $env:FIXTURE_STAGING = Join-Path $work 'staging'
    $log = Join-Path $work 'fixture.log'
    $env:FIXTURE_LOG = $log
    if ((Invoke-Checked 'fixture update' $exe @('update')).Trim() -ne 'up to date') { Fail '1.0.0 found an update in its own release' }
    Pass '1.0.0 is up to date with 1.0.0 in the feed'

    Copy-Item -Path (Join-Path $work 'feed-2.0.0\*') -Destination $served -Force
    $said = Invoke-Checked 'fixture update' $exe @('update')
    if ($said -notmatch 'available 2\.0\.0') { Fail "1.0.0 did not hear of 2.0.0`n$said" }
    # The installer waits for 1.0.0 to leave, replaces the files, and starts
    # 2.0.0 with the arguments of /RELAUNCH=, which writes the log.
    $again = Wait-Until { (Test-Path -LiteralPath $log) -and ((Get-Content -LiteralPath $log -Raw).Trim() -eq '2.0.0') } 60
    if (-not $again) {
        $found = if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Raw } else { 'no log' }
        Fail "2.0.0 did not start again after the update ($found)"
    }
    if ((Invoke-Checked 'fixture version' $exe @('version')).Trim() -ne '2.0.0') { Fail 'the installed application is not 2.0.0' }
    if ((Invoke-Checked 'fixture helper' $exe @('helper')).Trim() -ne 'helper 2.0.0') { Fail 'the installed helper is not 2.0.0' }
    if ((Get-ItemProperty -LiteralPath $uninstallKey).DisplayVersion -ne '2.0.0') { Fail 'the uninstall entry is not 2.0.0' }
    if ((Invoke-Checked 'fixture update' $exe @('update')).Trim() -ne 'up to date') { Fail '2.0.0 found an update in its own release' }
    Pass '1.0.0 updated itself to 2.0.0 through the installer, and started again'

    # Uninstalling: the uninstaller copies itself aside and returns at once.
    Start-Process -FilePath (Join-Path $install 'uninstall.exe') -ArgumentList '/S' -Wait
    $gone = Wait-Until { -not (Test-Path -LiteralPath $install) } 30
    if (-not $gone) {
        $left = (Get-ChildItem -LiteralPath $install -Recurse -Force | ForEach-Object { $_.FullName }) -join ', '
        Fail "uninstalling left $left"
    }
    if (Test-Path -LiteralPath $uninstallKey) { Fail 'uninstalling left the uninstall entry' }
    if (Test-Path -LiteralPath $schemeKey) { Fail 'uninstalling left the URL scheme' }
    if (Test-Path -LiteralPath "HKCU:\Software\Classes\$progId") { Fail 'uninstalling left the document type' }
    if ((Get-Default $extensionKey) -eq $progId) { Fail 'uninstalling left .rcfixture pointing at the application' }
    if (Test-Path -LiteralPath $shortcut) { Fail 'uninstalling left the shortcut' }
    Pass 'uninstalling removes the files, the shortcut and the registry entries'
}
finally {
    if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue }
    try { Remove-Installed } catch { Write-Host "could not remove the installed fixture: $_" }
    if ($env:KEEP -eq '1') {
        Write-Host "kept $work"
    }
    else {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
