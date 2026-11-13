# Builds the Windows installer of the product, under the product's own names
# (crates/branding/brand.env), on the `stable` channel, with nothing uploaded
# anywhere. Written for this fork.
#
# The installer is made with Inno Setup 6 (ISCC.exe) from
# crates\ide\resources\windows\installer.iss. It installs for the current user,
# without administrator rights. Nothing is signed, so Windows SmartScreen asks
# before the installer starts ("More info", then "Run anyway").
#
# What it writes, into target\release:
#
#   <APP_ID>-windows-x86_64-setup.exe      the installer
#   <APP_ID>-windows-x86_64\<PRODUCT_NAME>\   the folder the installer packs:
#     <APP_ID>.exe                         the editor (cargo's unaligned-ide.exe)
#     bin\<APP_ID>.exe                     the command (cargo's cli.exe)
#     conpty.dll, OpenConsole.exe          the terminal's console host, when the build fetched it
#
# Rules and what was never executed: docs/unaligned/PACKAGING.md.

[CmdletBinding()]
Param(
    # Do not build: package the binaries already in the target directory.
    [switch]$PackageOnly,
    # Stop after the folder: do not look for Inno Setup and write no installer.
    [switch]$NoInstaller
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..\..')

function Fail([string]$message) {
    Write-Host "bundle-windows.ps1: $message"
    exit 1
}

# Runs a program and stops the script when it fails.
function Run([string]$program, [string[]]$arguments) {
    Write-Host "> $program $($arguments -join ' ')"
    & $program @arguments
    if ($LASTEXITCODE -ne 0) {
        Fail "$program ended with code $LASTEXITCODE"
    }
}

# --- Brand ---------------------------------------------------------------------

# The brand file is plain KEY="value" lines. Anything else is refused.
$brandFile = 'crates\branding\brand.env'
$brand = @{}
$number = 0
foreach ($line in Get-Content $brandFile) {
    $number++
    if ($line -eq '' -or $line.StartsWith('#')) { continue }
    if ($line -notmatch '^([A-Z][A-Z0-9_]*)="([^"\\$`]*)"$') {
        Fail "${brandFile}:$number is not a KEY=`"value`" line"
    }
    $brand[$Matches[1]] = $Matches[2]
}
foreach ($key in @('PRODUCT_NAME', 'APP_ID', 'PUBLISHER', 'WEBSITE_URL', 'URL_SCHEME', 'WINDOWS_INSTALLER_GUID')) {
    if ([string]::IsNullOrWhiteSpace($brand[$key])) { Fail "$key is empty or missing in $brandFile" }
}
$productName = $brand['PRODUCT_NAME']
$appId = $brand['APP_ID']
if ($appId -eq 'zed') { Fail 'APP_ID must not be "zed": the build would collide with an installed Zed' }
if ($appId -notmatch '^[a-z0-9][a-z0-9-]*$') { Fail 'APP_ID must be lowercase letters, digits and dashes (it names files)' }
if ($productName -match '[<>:"/\\|?*]') { Fail 'PRODUCT_NAME must not contain a character Windows refuses in file names' }

# --- Preconditions -------------------------------------------------------------

if ($env:RUSTFLAGS -or $env:CARGO_ENCODED_RUSTFLAGS) {
    Fail 'unset RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS: they replace the flags of .cargo\config.toml'
}
# A build that sees the first variable sends crash reports to that address; the
# second one signs telemetry requests. Neither belongs in this product.
foreach ($forbidden in @('IDE_MINIDUMP_ENDPOINT', 'IDE_CLIENT_CHECKSUM_SEED')) {
    if (Test-Path "env:$forbidden") { Fail "$forbidden is set in the environment: unset it, a bundle must never be built with it" }
}

$target = 'x86_64-pc-windows-msvc'
$arch = 'x86_64'
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$buildDir = Join-Path $targetDir "$target\release"
$outDir = Join-Path $targetDir 'release'

$versionLine = Select-String -Path 'crates\ide\Cargo.toml' -Pattern '^version = "(.*)"$' | Select-Object -First 1
if (-not $versionLine) { Fail 'could not read the version of the ide package' }
$version = $versionLine.Matches[0].Groups[1].Value

# --- Build ---------------------------------------------------------------------

$channelFile = 'crates\ide\RELEASE_CHANNEL'
$channelBefore = $null

try {
    if (-not $PackageOnly) {
        # IDE_BUNDLE is left unset on purpose, as in bundle-mac.
        $env:RELEASE_VERSION = $version
        if (-not $brand['RELEASES_REPOSITORY'] -and -not (Test-Path 'env:IDE_UPDATE_EXPLANATION')) {
            $env:IDE_UPDATE_EXPLANATION = "This build of $productName has no releases page to check. Get a newer build from where you got this one."
        }
        if (-not (Test-Path 'assets\licenses.md')) {
            Write-Host 'bundle-windows.ps1: warning: assets\licenses.md is missing (script/generate-licenses writes it): the licences page of this build will say so. Do not distribute it.'
        }

        # Names and update behaviour follow the channel compiled in: build as
        # `stable`, then put the file back.
        $current = (Get-Content $channelFile -Raw).Trim()
        if ($current -ne 'stable') {
            $channelBefore = Get-Content $channelFile -Raw
            Set-Content -Path $channelFile -Value 'stable' -NoNewline
        }
        # The build script of the ide package picks the icon from these.
        $env:RELEASE_CHANNEL = 'stable'
        $env:IDE_RELEASE_CHANNEL = 'stable'

        Run 'rustup' @('target', 'add', $target)

        # Two invocations, as upstream: one feature set per binary. No call/webrtc.
        $cargoConfig = @('--config', '.cargo/bundle-config.toml', '--config', 'profile.release.debug=false')
        Run 'cargo' ($cargoConfig + @('build', '--release', '--locked', '--target', $target, '--package', 'ide', '--bin', 'unaligned-ide'))
        Run 'cargo' ($cargoConfig + @('build', '--release', '--locked', '--target', $target, '--package', 'cli'))
    }
}
finally {
    if ($null -ne $channelBefore) {
        Set-Content -Path $channelFile -Value $channelBefore -NoNewline
        Write-Host "Restored $channelFile."
    }
}

$editor = Join-Path $buildDir 'unaligned-ide.exe'
$command = Join-Path $buildDir 'cli.exe'
if (-not (Test-Path $editor)) { Fail "$editor not found: build first (run without -PackageOnly)" }
if (-not (Test-Path $command)) { Fail "$command not found: build first (run without -PackageOnly)" }

# --- Folder --------------------------------------------------------------------

$stageRoot = Join-Path $outDir "$appId-windows-$arch"
$stage = Join-Path $stageRoot $productName
if (Test-Path $stageRoot) { Remove-Item -Path $stageRoot -Recurse -Force }
New-Item -Path (Join-Path $stage 'bin') -ItemType Directory -Force | Out-Null

Copy-Item -Path $editor -Destination (Join-Path $stage "$appId.exe")
Copy-Item -Path $command -Destination (Join-Path $stage "bin\$appId.exe")
# The editor looks for its command as bin\<APP_ID>.exe (crates/util/src/util.rs);
# the command looks for the editor as ..\<APP_ID>.exe.

# The build script of the ide package fetches these two beside the binaries.
# Without them the terminal still works, with the console host of Windows.
foreach ($name in @('conpty.dll', 'OpenConsole.exe')) {
    $source = Join-Path $buildDir $name
    if (Test-Path $source) {
        Copy-Item -Path $source -Destination (Join-Path $stage $name)
    } else {
        Write-Host "bundle-windows.ps1: note: $name was not found in $buildDir; it is left out."
    }
}

foreach ($name in @('LICENSE-GPL', 'LICENSE-APACHE')) {
    if (Test-Path $name) { Copy-Item -Path $name -Destination (Join-Path $stage $name) }
}
if (Test-Path 'assets\licenses.md') { Copy-Item -Path 'assets\licenses.md' -Destination (Join-Path $stage 'licenses.md') }

# The command answers under the product's name, and names the editor it found
# beside itself. Information only: this question must not lose a finished build.
try {
    $said = & (Join-Path $stage "bin\$appId.exe") --version 2>&1
    Write-Host "The command says (exit code $LASTEXITCODE): $($said | Select-Object -First 1)"
} catch {
    Write-Host "bundle-windows.ps1: warning: could not run the command to ask its version: $_"
}

Write-Host ''
Write-Host "Folder:    $stage"
if ($NoInstaller) { exit 0 }

# --- Installer -------------------------------------------------------------------

if ($brand['WINDOWS_INSTALLER_GUID'] -notmatch '^[0-9A-Fa-f]{8}(-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$') {
    Fail "WINDOWS_INSTALLER_GUID in $brandFile is not a GUID (8-4-4-4-12 hexadecimal digits, no braces)"
}

# Inno Setup's compiler: on the PATH, or where its own installer puts it.
$iscc = $null
$onPath = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
if ($onPath) { $iscc = $onPath.Source }
if (-not $iscc) {
    foreach ($root in @(${env:ProgramFiles(x86)}, $env:ProgramFiles, (Join-Path $env:LOCALAPPDATA 'Programs'))) {
        if (-not $root) { continue }
        $candidate = Join-Path $root 'Inno Setup 6\ISCC.exe'
        if (Test-Path $candidate) { $iscc = $candidate; break }
    }
}
if (-not $iscc) {
    Fail 'Inno Setup 6 (ISCC.exe) was not found. Install it (winget install JRSoftware.InnoSetup, or https://jrsoftware.org/isdl.php), or pass -NoInstaller to stop after the folder.'
}

$setupName = "$appId-windows-$arch-setup"
$setup = Join-Path $outDir "$setupName.exe"
if (Test-Path $setup) { Remove-Item -Path $setup -Force }

# Paths in the .iss file are read relative to that file: give absolute ones.
$defines = @(
    "/DAppGuid=$($brand['WINDOWS_INSTALLER_GUID'])",
    "/DAppName=$productName",
    "/DAppExeName=$appId.exe",
    "/DAppVersion=$version",
    "/DAppPublisher=$($brand['PUBLISHER'])",
    "/DAppUrl=$($brand['WEBSITE_URL'])",
    "/DAppMutex=$appId-Stable-Instance-Mutex",
    "/DUrlScheme=$($brand['URL_SCHEME'])",
    "/DSourceDir=$((Resolve-Path $stage).Path)",
    "/DOutputDir=$((Resolve-Path $outDir).Path)",
    "/DOutputBaseFilename=$setupName",
    "/DIconFile=$((Resolve-Path 'crates\ide\resources\windows\app-icon.ico').Path)"
)
Run $iscc ($defines + @('/Qp', (Resolve-Path 'crates\ide\resources\windows\installer.iss').Path))
if (-not (Test-Path $setup)) { Fail "Inno Setup ended without writing $setup" }

Write-Host "Installer: $setup ($([math]::Round((Get-Item $setup).Length / 1MB)) MB)"
