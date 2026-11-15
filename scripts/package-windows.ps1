# Builds the Windows release of Working and its per-user NSIS installer:
#   target\dist\Working_<version>_<arch>-setup.exe   (<arch>: x64 or arm64, the machine this runs on)
# That raw installer is what latest.json points the updater at (platform keys windows-x86_64 / windows-aarch64,
# format "nsis"); cargo-packager-updater runs it with /P /R (passive, then relaunch the app).
# With CARGO_PACKAGER_SIGN_PRIVATE_KEY (and CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD) set, it also writes the
# installer's minisign signature, <installer>.sig, which the updater checks against the public key in updater.rs.
# The installer and the exe are not Authenticode-signed: the project has no Windows code-signing certificate.
#
# Release builds check for updates only when WORKING_UPDATES=1 is set for the build, as the release workflow does.
# Needs the MSVC Rust toolchain, clang on the PATH (ring builds C with it on arm64), and cargo-packager 0.11.8:
#   cargo install cargo-packager --version 0.11.8 --locked
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
$root = (Get-Location).Path

$meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
$version = ($meta.packages | Where-Object name -eq 'working').version
$release = Join-Path $meta.target_directory 'release'
$dist = Join-Path $root 'target\dist'
$icon = Join-Path $root 'crates\app\assets\working.ico'

cargo build --release --locked -p working
if ($LASTEXITCODE) { exit $LASTEXITCODE }

# Windows 11 ships the WebView2 runtime, and Windows 10 gets it through Windows Update; cargo-packager 0.11.8's NSIS
# template installs nothing for it. Where it is missing, this section fetches Microsoft's Evergreen bootstrapper and
# installs the runtime for the current user before the app is copied.
$webview2 = @'
Section WebView2
  StrCpy $4 ""
  ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${If} $4 == ""
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${EndIf}
  ${If} $4 == ""
    ReadRegStr $4 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${EndIf}
  ${If} $4 == ""
  ${OrIf} $4 == "0.0.0.0"
    DetailPrint "Installing the WebView2 runtime..."
    nsExec::ExecToLog 'powershell.exe -NoProfile -NonInteractive -Command "Invoke-WebRequest -UseBasicParsing -Uri https://go.microsoft.com/fwlink/p/?LinkId=2124703 -OutFile $\"$TEMP\MicrosoftEdgeWebview2Setup.exe$\""'
    Pop $5
    ExecWait '"$TEMP\MicrosoftEdgeWebview2Setup.exe" /silent /install' $5
    Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
    ${If} $5 != 0
      MessageBox MB_ICONSTOP "Working needs the Microsoft WebView2 runtime, which could not be installed (error $5)."
      Abort
    ${EndIf}
  ${EndIf}
SectionEnd
'@

# The identifier is also the AppUserModelID the template gives the Start Menu shortcut (ApplicationID::Set): the same
# id the app sets on itself (notify::windows::APP_ID), so Windows shows its toasts and groups its windows under it.
$config = @{
  # Given here: without it cargo-packager 0.11.8 looks for the package name by changing into the config file's path as
  # if it were a folder, and fails with "The directory name is invalid".
  name = 'working'
  productName = 'Working'
  version = $version
  identifier = 'dev.felipefdl.working'
  publisher = 'felipefdl'
  description = 'Google Chat on your desktop'
  homepage = 'https://github.com/felipefdl/working'
  binariesDir = $release
  binaries = @(@{ path = 'working.exe'; main = $true })
  formats = @('nsis')
  outDir = $dist
  icons = @($icon)
  nsis = @{
    installMode = 'currentUser'
    installerIcon = $icon
    preinstallSection = $webview2
    # Offered as "Delete the application data" when uninstalling: settings and session (roaming), and the cache and
    # webview data (local, apart from the install folder).
    appdataPaths = @('$APPDATA\working', '$LOCALAPPDATA\dev.felipefdl.working')
  }
}
New-Item -ItemType Directory -Force $dist | Out-Null
$configPath = Join-Path $dist 'packager.json'
# Without a byte order mark, which Windows PowerShell's UTF-8 Set-Content adds and JSON parsers refuse.
[IO.File]::WriteAllText($configPath, ($config | ConvertTo-Json -Depth 5))
cargo packager --release --config $configPath
if ($LASTEXITCODE) { exit $LASTEXITCODE }
Remove-Item $configPath

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
# cargo-packager names the installer after the binary (working_...), and with CARGO_PACKAGER_SIGN_PRIVATE_KEY set it
# signs it while packaging (.sig beside it). The release assets carry the product's name. The file system ignores case,
# so each rename goes through a temporary name.
$built = Join-Path $dist "working_${version}_$arch-setup.exe"
$installer = Join-Path $dist "Working_${version}_$arch-setup.exe"
if (-not (Test-Path $built)) { throw "cargo-packager did not write $built" }
foreach ($suffix in @('', '.sig')) {
  if (Test-Path "$built$suffix") {
    [IO.File]::Move("$built$suffix", "$installer$suffix.tmp")
    [IO.File]::Move("$installer$suffix.tmp", "$installer$suffix")
  }
}
if ($env:CARGO_PACKAGER_SIGN_PRIVATE_KEY -and -not (Test-Path "$installer.sig")) {
  throw "cargo-packager did not sign $installer"
}
Get-ChildItem $dist | Select-Object Name, Length
