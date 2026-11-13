$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$CARGO_ABOUT_VERSION="0.8.2"
$outputFile=$args[0] ? $args[0] : "$(Get-Location)/assets/licenses.md"
$templateFile="script/licenses/template.md.hbs"

# The application's licences page shows this file as the complete list of
# third-party notices, so it is built beside its destination and moved into
# place only when every block is in it: a run that stops half way leaves no
# partial file behind.
$partialFile = "$outputFile.partial"
$dependenciesFile = "$outputFile.dependencies"

# The "FONT LICENSES" block: for every font family under assets/fonts, a
# heading with the family's name followed by the licence files found in its
# directory (the SIL Open Font License asks that the copyright notice and the
# licence accompany the fonts, and the fonts are embedded in the binary).
# Fails when a family has no licence file. script/licenses/font-licenses holds
# the same logic for the shell script: change both.
function Get-FontLicenseLines {
    param([string]$FontsDir = "assets/fonts")

    # The family's name as its authors write it. A directory that is not
    # listed is printed under its own name.
    $familyNames = @{
        'ibm-plex-sans'  = 'IBM Plex Sans'
        'jetbrains-mono' = 'JetBrains Mono'
        'lilex'          = 'Lilex'
        'onest'          = 'Onest'
        'unbounded'      = 'Unbounded'
    }

    $families = @(Get-ChildItem -Path $FontsDir -Directory | Sort-Object -Property Name)
    if ($families.Count -eq 0) {
        throw "generate-licenses: no font family under $FontsDir"
    }

    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add("# ###### FONT LICENSES ######")
    foreach ($family in $families) {
        $licenseFiles = @(Get-ChildItem -Path $family.FullName -File |
            Where-Object { $_.Name -match '^(OFL|LICENSE|LICENCE|COPYING)' } |
            Sort-Object -Property Name)
        if ($licenseFiles.Count -eq 0) {
            throw "generate-licenses: $($family.FullName) has no licence file (OFL.txt, LICENSE*, license.txt)"
        }

        $name = $familyNames.ContainsKey($family.Name) ? $familyNames[$family.Name] : $family.Name
        $lines.Add("")
        $lines.Add("## $name")
        foreach ($licenseFile in $licenseFiles) {
            $lines.Add("")
            foreach ($line in @(Get-Content -LiteralPath $licenseFile.FullName -Encoding utf8)) {
                $lines.Add($line)
            }
        }
        $lines.Add("")
        $lines.Add("********************************************************************************")
    }
    return $lines
}

try {
    New-Item -Path "$partialFile" -ItemType File -Value "" -Force

    @(
        "# ###### THEME LICENSES ######"
        ""
        Get-Content assets/themes/LICENSES
        ""
        "# ###### ICON LICENSES ######"
        ""
        Get-Content assets/icons/LICENSES
        ""
        Get-FontLicenseLines "assets/fonts"
        ""
        "# ###### CODE LICENSES ######"
        ""
    ) | Add-Content -Path $partialFile

    $needsInstall = $false
    try {
        $versionOutput = cargo about --version
        if (-not ($versionOutput -match "cargo-about $CARGO_ABOUT_VERSION")) {
            $needsInstall = $true
        } else {
            Write-Host "cargo-about@$CARGO_ABOUT_VERSION is already installed"
        }
    } catch {
        $needsInstall = $true
    }

    if ($needsInstall) {
        Write-Host "Installing cargo-about@$CARGO_ABOUT_VERSION..."
        cargo install "cargo-about@$CARGO_ABOUT_VERSION"
    }

    Write-Host "Generating cargo licenses"

    # cargo-about replaces the file it is given with `-o`, so it writes the
    # dependency block to a file of its own, which is then appended: the theme,
    # icon and font blocks above stay in the result.
    $failFlag = $env:ALLOW_MISSING_LICENSES ? "--fail" : ""
    $aboutArgs = @('about', 'generate', $failFlag, '-c', 'script/licenses/licenses.toml', $templateFile, '-o', $dependenciesFile) | Where-Object { $_ }
    cargo @aboutArgs
    Get-Content -LiteralPath $dependenciesFile -Encoding utf8 | Add-Content -Path $partialFile

    Write-Host "Applying replacements"
    $replacements = @{
        '&quot;' = '"'
        '&#x27;' = "'"
        '&#x3D;' = '='
        '&#x60;' = '`'
        '&lt;'   = '<'
        '&gt;'   = '>'
    }
    $content = Get-Content $partialFile
    foreach ($find in $replacements.keys) {
        $content = $content -replace $find, $replacements[$find]
    }
    $content | Set-Content $partialFile

    Move-Item -LiteralPath $partialFile -Destination $outputFile -Force
} finally {
    Remove-Item -LiteralPath $partialFile, $dependenciesFile -Force -ErrorAction SilentlyContinue
}

Write-Host "generate-licenses completed. See $outputFile"
