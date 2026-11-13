# Packaging: what exists, the rules, and what was never executed

State on 2026-11-13, after the names inherited from upstream were replaced (`script/unaligned/rename-upstream-names`: cargo package `ide`, binary `unaligned-ide`, `IDE_…` variables), upstream's own packaging files were deleted, and the packages became files a user opens directly: an installer for Windows, a disk image for macOS, an AppImage for Linux. No script writes a zip any more.

| Piece | What it is |
| --- | --- |
| Linux packages: `script/unaligned/bundle-linux` | an `.AppImage` (one file that runs where it is) and a `.tar.gz` of the application directory, with `SHA256SUMS` for both and a filled-in `install.sh` |
| Linux desktop entry: `crates/ide/resources/unaligned-ide.desktop.in` | the template `bundle-linux` renders |
| Linux installer: `script/unaligned/install.sh` | installs the `.tar.gz` for the current user, without root |
| Windows installer: `script/unaligned/bundle-windows.ps1` | stages a folder, then compiles it into `<APP_ID>-windows-x86_64-setup.exe` with Inno Setup 6; per user, not signed. **Never run**: written on a Mac |
| Windows installer definition: `crates/ide/resources/windows/installer.iss` | the Inno Setup script the line above compiles. **Never compiled** |
| macOS disk image: `script/unaligned/bundle-mac` | a `.dmg` with the `.app` and a link to `/Applications`; ad hoc signature, no notarisation |
| Test builds on GitHub: `.github/workflows/build.yml` | runs the three scripts and keeps the results as artifacts of the run; a run started by a tag also attaches the installer, the disk images and the AppImage to a draft release; see `.github/workflows/README.md` |
| Exe metadata: `crates/windows_resources` | product name per channel and company name from the brand file; the copyright line keeps upstream's notice and adds the fork's |
| `.github/workflows/release.yml` | **not written**: there is no automated release |

Read "Executed and not executed" at the end before trusting any of it.

## Three rules that come before the first release

1. **Tag `v<version of the ide package>`.** The `ide` package is at 0.1.0 (`crates/ide/Cargo.toml`; ruling G5). The update check compares the running version with the newest release tag, and the release-notes link is `…/releases/tag/v<running version>`: a release whose tag is not exactly the version with a leading `v` would never be announced, and its notes link would be dead. `branding::UPSTREAM_VERSION` keeps 1.30.0 for the credit line.
2. **Remote development needs server builds the product does not publish.** Outside a development build, opening an SSH, WSL or container project works only when a matching server binary is already on the host; otherwise it fails with "Unaligned IDE does not publish remote server builds yet, so there is none to download for <os> <arch>" (ruling G6: accepted for now). To restore it: build `remote_server` in the release job (a separate cargo invocation for the musl target, then `gzip`, as upstream's Linux bundle script did), publish it as a release asset, and return its address from `get_remote_server_release_url` in `crates/auto_update`. This is the first follow-up after a working release. `bundle-linux` here does not build it.
3. **Never set `IDE_MINIDUMP_ENDPOINT` or `IDE_CLIENT_CHECKSUM_SEED`** in a bundle script, a workflow or the shell that runs one. Both are read at compile time (and again at run time): a build that sees the first one uploads crash reports to that address; the second signs telemetry requests. The three bundle scripts refuse to start when either is present in the environment. A future `release.yml` must never define them.

## File names inside the packages

The command (cargo's `cli`) and the editor (cargo's `unaligned-ide`) find each other by file name relative to their own location. Both names are built from `branding::APP_ID`. There are no copies or links under upstream's names any more.

| System | The editor | The command | Where the lookups are |
| --- | --- | --- | --- |
| Linux | `libexec/<APP_ID>-editor` | `bin/<APP_ID>` | the command tries `../libexec/<APP_ID>-editor`, then `../lib/<APP_ID>/<APP_ID>-editor`, then `./<APP_ID>` (`crates/cli/src/main.rs`); the editor tries `../bin/<APP_ID>`, then `./cli` (`crates/util/src/util.rs`, `get_zed_cli_path(app_id)`) |
| Windows | `<APP_ID>.exe` | `bin\<APP_ID>.exe` | the command tries `..\<APP_ID>.exe`, then `.\<APP_ID>.exe`; the editor tries `bin\<APP_ID>.exe`, then `.\cli.exe` |
| macOS | `Contents/MacOS/<APP_ID>` (the bundle's `CFBundleExecutable`) | `Contents/MacOS/cli` | the command walks up to the enclosing `.app` and starts `Contents/MacOS/<APP_ID>`; the editor tries `./cli` |

`<command> --app <path>` names the editor (or, on macOS, the `.app`) directly and skips the lookup. The flag was `--zed` before the rename.

In a development build on Linux or Windows the command finds the editor as `./<APP_ID>` beside itself in the target directory. That works only while the cargo binary name equals `APP_ID`: the binary name `unaligned-ide` is written in `crates/ide/Cargo.toml`, in the three bundle scripts and in the tooling that starts `target/debug/unaligned-ide` (`shot`, `netcheck`, `build-offscreen`), and does not follow the brand file. If `APP_ID` changes, rename the binary in those places too. On macOS a development build has no `.app` around it: pass `--app target/debug/unaligned-ide`.

## Cutting a Linux release by hand (the only way today)

On a Linux machine of the architecture you want to ship (the archive is named after `uname -m`):

1. In `crates/branding/brand.env`, set `RELEASES_REPOSITORY` (an `owner/repo` slug on GitHub, not upstream's) and `SOURCE_REPOSITORY_URL`. Commit.
2. Apply rule 1, tag `v<version>`, push the branch and the tag. The repository must be public and hold that tag: it is the source offer the GPL asks for (see below).
3. `script/linux` (build dependencies; it still installs `libva`, which the default build no longer needs since WebRTC is opt-in: harmless).
4. `script/unaligned/bundle-linux`. The tools it calls itself are `cargo`, `rustc`, `jq`, `ldd`, `tar`, `gzip`, `sha256sum` and `llvm-objcopy` (or `objcopy`), and for the AppImage `curl` and `file`, with network access for two downloads (see the script's section below). `--no-appimage` writes the archive only and needs neither. `script/generate-licenses` compiles and installs `cargo-about` on first use (several minutes).
5. Create the GitHub release for the tag and attach the four files it wrote in `target/release/`: `unaligned-ide-linux-<arch>.AppImage`, `unaligned-ide-linux-<arch>.tar.gz`, `SHA256SUMS` (it lists both), `install.sh`. If you ship two architectures, concatenate their `SHA256SUMS` into one file.

Users then either download the AppImage, make it executable and start it:

```sh
chmod +x unaligned-ide-linux-x86_64.AppImage
./unaligned-ide-linux-x86_64.AppImage
```

or install the archive with:

```sh
curl -fsSL https://github.com/<owner>/<repo>/releases/latest/download/install.sh | sh
```

`install.sh` installs the `.tar.gz` only; it does not know the AppImage.

## `script/unaligned/bundle-linux`

Adapted from upstream's bundle script for Linux, which is no longer in the repository. All names come from `crates/branding/brand.env` (validated line by line, then sourced).

What it does, in order:

1. Refuses to run with `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS` set (they would replace the bundle's library path), with either variable of rule 3 set, or on a system that is not Linux.
2. Sets `IDE_BUNDLE=true` and `RELEASE_VERSION=<version of the ide package>`. When the brand file has no `RELEASES_REPOSITORY` it also sets `IDE_UPDATE_EXPLANATION` to a sentence that names the product: "Check for Updates" then shows that sentence instead of checking (the updater is already inert in that case; this is belt and braces). A packager who sets the variable himself keeps his text.
3. Runs `script/generate-licenses`, which writes `assets/licenses.md` (git-ignored). The app's licences page embeds that file, so it must exist before the build.
4. Writes `stable` into `crates/ide/RELEASE_CHANNEL` and puts the original file back, with its modification time, when the script ends, fails or is interrupted. Only `kill -9` can leave the file at `stable`: `git status` shows it.
5. Builds with `--config .cargo/bundle-config.toml --config profile.release.debug=false`, `--release --locked --target <host triple>`: first `--package ide --bin unaligned-ide`, then `--package cli` (two invocations, as upstream, so that each binary gets its own feature set). No `--features`: in particular never `call/webrtc`.
6. Stages the application directory, strips the two binaries, copies the libraries `ldd` reports (upstream's exclusion list: never the C library, ALSA, or the GLib family), the icons, the desktop entry, the licences.
7. Runs the staged command with `--version`. That goes through the same lookup as a normal start, so the script fails here if the command cannot find the editor.
8. Writes the archive `<APP_ID>-linux-<arch>.tar.gz`.
9. Unless `--no-appimage` is given, writes `<APP_ID>-linux-<arch>.AppImage` (next section).
10. Writes `SHA256SUMS` (the archive, and the AppImage when there is one) and a copy of `install.sh` with the brand values filled in.

Options: `--package-only` (skip steps 2 to 5 and package what is already in the target directory), `--no-appimage` (skip step 9: no download, no `curl`, no `file`), `--print-desktop` and `--print-install` (print the rendered desktop entry or installer and exit; neither builds anything, and both work on any system).

The archive:

```
unaligned-ide.app/
  bin/unaligned-ide                       the command (cargo's `cli`)
  libexec/unaligned-ide-editor            the editor (cargo's `unaligned-ide`)
  lib/                                    bundled libraries (found through the binaries' rpath `$ORIGIN/../lib`)
  share/applications/pw.unaligned.ide.desktop
  share/icons/hicolor/512x512/apps/unaligned-ide.png
  share/icons/hicolor/1024x1024/apps/unaligned-ide.png
  licenses.md  LICENSE-GPL  LICENSE-APACHE
```

Nothing in the archive and nothing `install.sh` writes carries upstream's name. The two compatibility links of the first version of this script (`bin/zed`, `libexec/zed-editor`) are gone, with the lookups that needed them.

### The AppImage

One file the user makes executable and starts; nothing is installed. Step 9 in detail:

- It downloads two files with `curl` into a temporary directory and checks each against a SHA-256 value written in the script; a mismatch stops the script and nothing is packed:
  - `appimagetool` 1.9.1, from `github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-<arch>.AppImage`;
  - the static runtime `type2-runtime` 20251108, from `github.com/AppImage/type2-runtime/releases/download/20251108/runtime-<arch>`. With the static runtime the user's machine needs no libfuse2.
- The script has checksums for `x86_64` and `aarch64`. On any other architecture it writes no AppImage and says nothing about it.
- It stops when the `file` command is missing (appimagetool needs it).
- The image holds a copy of the application directory of the archive, plus, at its root: `AppRun`, the desktop entry `pw.unaligned.ide.desktop`, the icon `unaligned-ide.png` (512 px) and a `.DirIcon` link to it.
- **`AppRun` starts the editor (`libexec/<APP_ID>-editor`) directly, with the arguments it was given, not the command.** The command returns at once, and an AppImage is unmounted when what it started returns. The command is still inside the image (`bin/<APP_ID>`) but nothing starts it.
- appimagetool is itself an AppImage: the script runs it with `APPIMAGE_EXTRACT_AND_RUN=1`, so the build machine needs no FUSE, and with `--no-appstream`.
- To update the tool or the runtime: change the version and the two checksums of each architecture together.

Known limits, none of them looked at:

- "Restart" from inside the application when it runs from an AppImage.
- The desktop entry inside the image names the command (`Exec=unaligned-ide %U`), as in the archive. Nothing here adds the AppImage to the desktop's menu or registers the `unaligned-ide://` handler for it: that is `install.sh`'s job, for the archive.
- `install.sh` does not know the AppImage. Whether the update notice makes sense for a copy started from an AppImage was not looked at either.

### Differences from upstream's script, on purpose

- No symbol upload, no debug companions (`debug = false`), no remote server, no Flatpak switch.
- The AppImage is an addition of this fork.
- The release link flags upstream adds for size (`-C link-arg=-Wl,--icf=safe` with an explicit `lld`) are left out: they add a hard dependency on a recent `lld`. A candidate once a release works.
- The desktop template uses `@NAME@` placeholders filled by the script itself instead of `envsubst`.
- Icons are installed under the product's name.

## The desktop entry

`crates/ide/resources/unaligned-ide.desktop.in`, rendered to `pw.unaligned.ide.desktop`. Its file name and `StartupWMClass` equal `ReleaseChannel::app_id()` for the stable channel (`branding::REVERSE_DNS_ID`): that is what ties the app's windows to the entry on Wayland and X11. A build on another channel has another id (`pw.unaligned.ide-Dev`, …) and would not match: the script only builds `stable`.

It declares `x-scheme-handler/unaligned-ide` (`branding::URL_SCHEME`). A link `unaligned-ide://…` therefore reaches the command as an argument, and the app rewrites it to its internal `ide://` form before parsing it (`internal_url` in `crates/ide/src/app/open_listener.rs`). `ide://`, `ide-cli://` and `ide-dock-action://` are internal: the app never registers them with the system. `bundle-linux` fails if the rendered entry contains the word "zed" or an unfilled placeholder.

## `script/unaligned/install.sh`

POSIX `sh`. Installs for the current user, without root:

- `~/.local/unaligned-ide.app` (replaced as a whole on each install),
- `~/.local/bin/unaligned-ide` (a link),
- `~/.local/share/applications/pw.unaligned.ide.desktop` (the entry with absolute paths for `Exec`, `TryExec` and `Icon`).

It writes nothing else, so an installed Zed (`~/.local/zed.app`, `~/.local/bin/zed`, its desktop entry) is not touched. To uninstall, remove those three paths; settings and data live in `~/.config/unaligned-ide` and `~/.local/share/unaligned-ide`. The command's own `--uninstall` is refused ("`--uninstall` is not available in Unaligned IDE yet. Nothing was removed.").

Behaviour worth knowing:

- The copy in the repository has empty brand lines and reads `crates/branding/brand.env`; the copy `bundle-linux` writes next to the archive has the values filled in, so it works when piped from `curl`. Attach that copy to the release, not the repository's.
- With `RELEASES_REPOSITORY` empty and no local archive it refuses with a message that says why and how to install a local build.
- The archive is checked against `SHA256SUMS` before anything on disk changes; a mismatch stops the install and leaves the previous version in place. The archive is unpacked beside the destination and swapped in at the end.
- An archive that holds anything outside `unaligned-ide.app/` is refused.
- `IDE_VERSION=v0.1.0` installs a given tag; `IDE_BUNDLE_PATH=<archive>` installs a local archive (verified when a `SHA256SUMS` sits beside it, otherwise installed unverified with a line that says so); `IDE_RELEASE_BASE_URL=<address>` downloads from a mirror.
- Downloads are restricted to `https` (and `file` for tests) when `curl` is used.

## Windows: `script/unaligned/bundle-windows.ps1`

**Never run.** The script and `crates/ide/resources/windows/installer.iss` were written on a Mac, which has neither PowerShell nor Inno Setup. Nothing in this section was executed; it describes what the two files say.

Builds for `x86_64-pc-windows-msvc`, stages the folder `target\release\unaligned-ide-windows-x86_64\Unaligned IDE\`, then compiles that folder into the installer `target\release\unaligned-ide-windows-x86_64-setup.exe` (`<APP_ID>-windows-x86_64-setup.exe`). It no longer writes a zip. The folder:

```
Unaligned IDE\
  unaligned-ide.exe                       the editor (cargo's `unaligned-ide.exe`)
  bin\unaligned-ide.exe                   the command (cargo's `cli.exe`)
  conpty.dll  OpenConsole.exe             the terminal's console host, when the build fetched them
  LICENSE-GPL  LICENSE-APACHE  licenses.md
```

Same rules as the Linux script: brand values from `brand.env` (`APP_ID` must not be `zed`), refusal when `RUSTFLAGS` or one of the two variables of rule 3 is set, `stable` written into `crates\ide\RELEASE_CHANNEL` for the build and put back afterwards, two cargo invocations, no `--features`. Differences:

- `IDE_BUNDLE` is not set (as in `bundle-mac`).
- It does not run the licence generator: it copies `assets\licenses.md` when the file is there and prints a warning when it is not ("Do not distribute it"). In `build.yml` a Linux job writes the file and hands it over.
- The staged command is asked for `--version` for information only: a failure does not stop the script.
- `-PackageOnly` packages the binaries already in the target directory.
- `-NoInstaller` stops after the folder: Inno Setup is not looked for and no installer is written.
- It also requires the brand keys `PUBLISHER`, `WEBSITE_URL`, `URL_SCHEME` and `WINDOWS_INSTALLER_GUID`, and refuses a `PRODUCT_NAME` with a character Windows does not allow in file names.

The installer step:

- It needs Inno Setup 6. The script looks for `ISCC.exe` on the `PATH`, then in `Inno Setup 6\` under `Program Files (x86)`, `Program Files` and `%LOCALAPPDATA%\Programs`. When it is not found the script stops and says how to install it (`winget install JRSoftware.InnoSetup`, or jrsoftware.org) or to pass `-NoInstaller`.
- It checks that `WINDOWS_INSTALLER_GUID` has the form of a GUID (8-4-4-4-12 hexadecimal digits, no braces).
- It runs `ISCC.exe` on `installer.iss` and passes every name on the command line (`/DName=value`): the GUID, the product name, `<APP_ID>.exe`, the version of the `ide` package, the publisher, the website, the mutex name, the link scheme, the staged folder, the output folder and file name, and the icon `crates\ide\resources\windows\app-icon.ico`. `installer.iss` is not meant to be compiled on its own: without the GUID it stops with an error that names the script.
- It fails when Inno Setup ends without having written the `-setup.exe`.

The folder and the installer do not contain `auto_update_helper.exe` (nothing in the product starts it) or the Explorer shell extension.

### The Windows installer: what it does, what it does not do, what is missing

Upstream's installer files (the Inno Setup script, its message files, the signing script, the WSL launcher `zed.sh`) were deleted from `crates/ide/resources/windows/`. That directory now holds the four icons and `installer.iss`, written for this product from nothing.

What `installer.iss` does:

- **Per user.** No administrator rights are asked (`PrivilegesRequired=lowest`); the folder offered is `%LOCALAPPDATA%\Programs\Unaligned IDE` (the product name).
- **Languages**: English and French, from the message files that come with Inno Setup.
- **Shortcuts**: a Start-menu entry, and a desktop icon the user may tick (not ticked by default).
- **Link scheme**: registers `unaligned-ide` (`URL_SCHEME`) for the current user under `Software\Classes`, with the command `"<folder>\unaligned-ide.exe" "%1"`. The key is removed on uninstall.
- **A running editor**: Setup asks the user to close it before replacing its files, through the mutex `unaligned-ide-Stable-Instance-Mutex`. That name must equal `ReleaseChannel::app_identifier()` + `-Instance-Mutex`, which the app builds from the brand in `crates/ide/src/app/windows_only_instance.rs`; the script builds it as `<APP_ID>-Stable-Instance-Mutex`.
- **Uninstaller**: Inno Setup's own, listed by Windows under the product's name.
- A last page offers to start the editor.
- It accepts 64-bit x86 Windows and systems that run such programs (`x64compatible`).
- The file names inside are those of the table above: `<APP_ID>.exe` and `bin\<APP_ID>.exe`.

Its identity is the brand key `WINDOWS_INSTALLER_GUID` in `crates/branding/brand.env` (Inno Setup's `AppId`), generated once for this product. **It must never change once a release is out**: an installer with another value installs a second copy beside the first instead of updating it. It must never be one of upstream's: Windows would treat the installer as a new version of an installed Zed and replace it.

What it does not do:

- It adds nothing to `Path`: the command `bin\unaligned-ide.exe` is installed but a terminal does not find it by name.
- It registers no file association and no Explorer menu entry ("Open with").
- It does not sign anything, and it installs no update mechanism.

Still missing:

- **A first run.** Neither the script nor the `.iss` was ever executed: compiling, installing, starting from the Start menu, opening an `unaligned-ide://` link, installing over a running editor, uninstalling.
- **Signing**: the installer and the two `.exe` inside it are unsigned (see "What is unsigned").
- **`Path`** for the command, and **file associations**.

Still carrying upstream's identifiers in the code, and not shipped: `crates/explorer_command_injector` (the "Open with Zed" Explorer menu: upstream's CLSIDs, its package identity in the three `AppxManifest*.xml` files, `Zed.exe`) and `crates/auto_update_helper` (`Zed.exe`, "Updating Zed..."). Do not put either in a package before replacing those. `check.yml` still type-checks both on Windows.

Also on Windows: the Credential Manager target prefix is `editor:url=` (`crates/gpui_windows/src/util.rs`; it was `zed:url=` before the rename, so a key stored by an earlier build is not found); the window class names are still `Zed::Window` and `Zed::PlatformWindow` (`crates/gpui_windows`, an Apache-2.0 crate that does not read the brand; not visible to a user).

## macOS: `script/unaligned/bundle-mac`

Builds `Unaligned IDE.app` for `aarch64-apple-darwin` or `x86_64-apple-darwin` (either on either kind of Mac), leaves it in `target/<triple>/release/bundle/`, and writes the disk image `target/release/<APP_ID>-macos-<arch>.dmg`. It no longer writes a zip. Same rules as the Linux script: brand values from `brand.env`, `stable` channel written for the build and put back afterwards, refusal when `RUSTFLAGS` or one of the two variables of rule 3 is set.

- `--runtime-shaders` is for a Mac without Xcode (the development Mac): the Metal shaders are compiled when the application starts instead of at build time.
- The bundle is assembled by hand (no `cargo-bundle`): `Contents/MacOS/<APP_ID>` (named as `CFBundleExecutable`) and `Contents/MacOS/cli`, an icon set made with `sips` and `iconutil` from `crates/ide/resources/app-icon@2x.png`, an `Info.plist` written by the script plus the three fragments of `crates/ide/resources/info/`. The plist declares the URL scheme `unaligned-ide`.
- `IDE_BUNDLE` is not set: with it the application expects its own `git` inside the bundle, which is not shipped.
- Like the Windows script it does not run the licence generator: it copies `assets/licenses.md` when the file is there and warns when it is not.
- `--package-only` does not build: it packages the binaries already in the target directory.
- The disk image holds the application and a link to `/Applications`: the user opens the image and drags one onto the other. The script copies the application with `ditto` (which keeps the signature), makes the image with `hdiutil create` (HFS+, compressed, the volume named after the product) and fails when `hdiutil verify` does not accept it. The image has no background picture and no window layout.
- The signature of the application is ad hoc (`codesign --sign -`, with `crates/ide/resources/app.entitlements`); the image itself is not signed. A downloaded copy must have its quarantine mark removed once it is in place: `xattr -dr com.apple.quarantine "/Applications/Unaligned IDE.app"`. A real release needs an Apple Developer identity and notarisation: not done.

## What is unsigned, and what users will see

- **Linux**: neither the archive nor the AppImage is signed. `SHA256SUMS` lists both; it comes from the same release page as the files, so it detects a corrupted or truncated download, not a compromised release page. Only `install.sh` checks it, and only for the archive: nothing checks a downloaded AppImage unless the user does. There is no GPG signature and no package repository.
- **Windows**: no code-signing certificate is planned. The installer and the two `.exe` it installs are unsigned. An unsigned installer makes SmartScreen show "Windows protected your PC" with the publisher "Unknown"; the user has to choose "More info", then "Run anyway". Browsers may also warn on download until the file has some reputation. Say this on the download page.
- **macOS**: test builds only; the application is signed ad hoc and the disk image is not signed (previous section). For the record: recent macOS draws an icon that is not Apple's rounded shape inside a grey tile in the Dock; nothing was done about it.

## Which Linux systems can run the archive (glibc floor)

The archive runs on systems whose glibc is at least the build machine's. The brief planned `ubuntu-22.04` runners (glibc 2.35). GitHub is retiring that image (slower queues since 2026-09-17, removed 2027-04-17), and `check.yml` and `build.yml` use `ubuntu-24.04`. For a release job the choices are:

| Build on | glibc floor | Runs on |
| --- | --- | --- |
| `ubuntu-24.04` runner | 2.39 | Ubuntu 24.04+, Debian 13+, Fedora 40+ |
| `ubuntu-24.04` runner with `container: ubuntu:22.04` | 2.35 | Ubuntu 22.04+, Debian 12+, Fedora 36+ |

Recommended: the container, for the wider floor. In a container the job runs as root: `script/linux` works as root, and `bundle-linux` already passes `IDE_ALLOW_ROOT=true` to its one run of the staged command (the command refuses root otherwise). Neither choice was tried.

## What is not shipped

- Remote server binaries (rule 2).
- A zip of the Windows folder or of the macOS application (until 2026-11-13 the only Windows and macOS packages).
- Flatpak, Snap, Nix, `.deb`, `.rpm`.
- Linux aarch64: the script handles it (names follow `uname -m`), but no build machine is planned.
- Debug symbols.

## Upstream's packaging files are gone

Until 2026-11-13 upstream's packaging, install and release scripts were kept in the tree and made to refuse. They are now deleted, with the templates they read: `script/bundle-*`, `script/install.sh`, `script/install-linux`, `script/uninstall.sh`, the upload, bump and trigger scripts, `script/flatpak/`, `script/snap-*`, `nix/` and the flake files, and in `crates/ide/resources/` the Flatpak and Snap templates, `zed.desktop.in` and the Inno Setup installer files.

- A merge of upstream brings these files back as "deleted by us, modified by them", or as new files without a conflict. Keep them deleted: each one acts under Zed's identity (names, application ids, URLs, storage buckets).
- Upstream's workflow generator (`cargo xtask workflows`) was deleted on 2026-11-13, so nothing writes its workflow files back into `.github/workflows/`. Should a merge bring some: `script/unaligned/check-branding` scans `.github/**` and fails on them, and the step "Only the fork's workflows" of `check.yml` fails on any file other than `check.yml`, `build.yml`, `release.yml` and `README.md`.

## GPL obligations

The app is GPL-3.0-or-later; parts of the code base are Apache-2.0.

- Met by the archive: `LICENSE-GPL`, `LICENSE-APACHE` and `licenses.md` (every dependency's licence, generated at bundle time) are inside it, and inside the AppImage, which holds the same directory; the licences page of the app shows the same texts; About credits upstream ("Based on Zed, © Zed Industries, Inc.").
- **Not met until `SOURCE_REPOSITORY_URL` is set**: About can only link to the source once the brand file names it, and a binary must not be published before the corresponding source is public at the same tag (GPL-3 section 6: offering the source from the same place as the binary is enough, which a public GitHub repository with the release tag provides).
- The licence list of a release must be complete: `script/generate-licenses` fails on a dependency whose licence is not accepted in `script/licenses/licenses.toml`, unless `ALLOW_MISSING_LICENSES` is set. `build.yml` sets that variable for its test builds; a release must be built without it.

## Test builds on GitHub: `.github/workflows/build.yml`

The workflow runs the three scripts on GitHub's machines (jobs `linux`, `macos-arm`, `macos-intel`, `windows`, and `licenses`, which writes `assets/licenses.md` for the macOS and Windows jobs). It starts by hand or by a tag named `build-…`; `.github/workflows/README.md` has the table of jobs and the instructions. What changed with the packages:

- **Artifacts.** The Linux job uploads the `.AppImage` with the `.tar.gz`, `SHA256SUMS` and `install.sh`; each macOS job uploads its `.dmg`; the Windows job uploads the `-setup.exe`. GitHub wraps each artifact in a zip of its own when it is downloaded from the run's page.
- **Linux job**: installs the `file` package before the build (appimagetool needs it).
- **Windows job**: installs Inno Setup with Chocolatey (`choco install innosetup`) only when `ISCC.exe` is not in `Program Files (x86)\Inno Setup 6`.
- **New job `draft-release`**, for runs started by a tag only. It downloads the artifacts of the run and attaches the `-setup.exe`, `.dmg` and `.AppImage` files, as they are (no zip around them), to a draft release named after the tag: `gh release create <tag> --draft --verify-tag`, titled "Test build <tag>". When a release of that name exists already, the files are uploaded to it and replace those of the same name. It runs even when the build of another system failed, and attaches what was built; it fails when no build produced a file. It is the only job with the permission `contents: write` (the workflow's own is `contents: read`). The archive, `SHA256SUMS` and `install.sh` are not attached: they stay in the Linux artifact.
- A draft release is seen only by people who can write to the repository, and is published by hand or not at all. A run started by hand makes no release: it only leaves artifacts.

`build.yml` sets `ALLOW_MISSING_LICENSES`, so its files are test builds, not releases (see "GPL obligations").

## What a future `release.yml` must do

Not written. `check.yml`'s guard step already allows a file of that name. Notes for whoever writes it:

- Trigger on tags `v*`; `permissions: contents: write`; no secret other than `GITHUB_TOKEN`; never define the variables of rule 3; do not set `RUSTFLAGS`; do not set `ALLOW_MISSING_LICENSES`.
- Pin actions by commit as `check.yml` does; install the toolchain with `rustup toolchain install --no-self-update`. `CARGO_PROFILE_DEV_*` variables do nothing for a release build.
- Linux job: see the glibc table; `script/linux` and the `file` package, then `script/unaligned/bundle-linux`, then upload the four files from `target/release/` (AppImage, archive, `SHA256SUMS`, `install.sh`).
- Windows job: Inno Setup 6 present (as the step of `build.yml` does), `script/unaligned/bundle-windows.ps1` with `assets\licenses.md` in place, then upload the `-setup.exe`.
- macOS, if it is released: `script/unaligned/bundle-mac <target>` with `assets/licenses.md` in place, then upload the `.dmg`.
- The job `draft-release` of `build.yml` shows one way to attach files to a release with `gh` and the job permission `contents: write`. It attaches only the installer, the disk images and the AppImage: a real release must also carry the archive, `SHA256SUMS` and `install.sh`, or the install command above and the checksum have nothing to read.
- Fail early when the tag is not `v<version of the ide package>` (rule 1) or when `RELEASES_REPOSITORY` is empty.
- A final job creates the release and merges the per-platform `SHA256SUMS` into one file (today only `bundle-linux` writes one).

## Candidates for removal after the first real link

`.cargo/config.toml` keeps two settings that exist for WebRTC, which the default build no longer contains: `+crt-static` for Windows and `-fuse-ld=lld` for aarch64 Linux. They were kept because nobody had linked on those targets without them. Once release builds work on those targets, try without them.

## Executed and not executed

In short, on 2026-11-13:

| Package | Executed? |
| --- | --- |
| Windows installer (`bundle-windows.ps1`, `installer.iss`) | **no, nothing**: both files were written on a Mac without PowerShell and without Inno Setup |
| macOS disk image (`bundle-mac`) | yes, on an Apple Silicon Mac: written, mounted, signature of the application inside verified. The installed application was **not** started |
| Linux AppImage (`bundle-linux`, step 9) | the packing section only, in the project's Linux container (aarch64), **on a stand-in application** |
| Linux archive and `install.sh` | not since the rename; before it, on stand-ins (table below) |
| The real build of the product for Linux or Windows | **never on this machine**. On GitHub: asked for by a tag pushed on 2026-11-13, result not known when this was written |

Not re-verified after the 2026-11-13 rename: `bundle-linux` as a whole and `bundle-windows.ps1` (the cargo package and binary they build, the file names they stage, the staged command finding the staged editor without the old compatibility links) and `install.sh` against an archive of the new layout. The Linux and Windows type-checks of the app and of the command pass with the new names (`check-linux`, `check-windows`, `-p ide` and `-p cli`).

What was recorded before the rename, on 2026-11-12, about `bundle-linux` and `install.sh` as they were then (the archive still had the two compatibility links):

| What | Executed? |
| --- | --- |
| `bash -n` on `bundle-linux`, `sh -n` on `install.sh` and on its rendered copy | yes, on the Mac |
| `shellcheck` | **no**: not installed on the Mac nor in the project's Docker image |
| `bundle-linux --print-desktop`, `--print-install`, `--help`, refusal on macOS | yes, on the Mac (bash 3.2) |
| `bundle-linux` guards (rule 3 variables, `RUSTFLAGS`, missing binaries) | yes, in the `unaligned-ide-check` image |
| `bundle-linux` build branch: channel file set to `stable` during the build and restored after a success and after a failure, variables exported, cargo arguments | yes, in the image, **with a stub `cargo` and a stub licence generator** that record what they are asked |
| Staging, stripping, library copy, archive, checksum | yes, in the image, **on stand-ins** (`curl` as the editor, GNU `true` as the command), aarch64 |
| The staged command finding the staged editor (step 7) | **no**: the stand-in only proved the step runs and fails the script on a non-zero exit |
| `desktop-file-validate` | **no**: not in the image; the script runs it when present |
| `install.sh`: refusal without a repository, install from a `file://` mirror when piped to `sh`, wrong checksum, local archive, home directory with a space, reinstall, archive with a foreign path | yes, in the image (Debian 13, as root), on the stand-in archive |
| `install.sh` against a real GitHub release (redirects, `releases/latest/download`), with `wget`, with `shasum`, on another distribution | **no** |
| The installed app starting from the menu entry, its icon, window grouping (`StartupWMClass`), the `unaligned-ide://` handler | **no**: needs a real Linux desktop |

### Windows: the script and the installer (2026-11-13)

`bundle-windows.ps1` and `crates/ide/resources/windows/installer.iss` were written on a Mac without PowerShell and without Inno Setup. **Neither has ever been run**: not the build, not the staging of the folder, not the search for `ISCC.exe`, not the compilation of the `.iss`, not the installer itself (install, Start-menu entry, desktop icon, the `unaligned-ide://` scheme, the question to close a running editor, the French pages, uninstall). The names the script passes to the `.iss` were compared by reading the two files, nothing more.

### macOS: the application and the disk image (2026-11-13)

`bundle-mac --runtime-shaders aarch64-apple-darwin` was run after the rename, when the script still wrote a zip: it built `Unaligned IDE.app` with `Contents/MacOS/unaligned-ide` and `Contents/MacOS/cli`, and the staged command answered `--version` and named the application it found. Starting that application was not confirmed: an older copy was open on the Mac, and the new one handed its request over to it and exited, as the single-instance rule says it should.

After the change to a disk image, `bundle-mac --package-only aarch64-apple-darwin` was run on the same Apple Silicon Mac: `unaligned-ide-macos-aarch64.dmg` was written (85 MB), the image was mounted, and the signature of the application inside it verified. Not tried: dragging the application to `/Applications` and starting the installed copy; a copy that went through a download (the quarantine mark and the `xattr` command); the Intel target.

### Linux: the AppImage (2026-11-13)

Only the packing section of `bundle-linux` (step 9) was run, in the project's Linux container (aarch64), on a stand-in application in place of the product:

- the two downloads (appimagetool 1.9.1 and the runtime 20251108, aarch64) matched the checksums written in the script;
- the AppImage was written;
- started with `--appimage-extract-and-run`, it ran the entry point (`AppRun`) with the arguments it was given.

Not done:

- The real build of the product for Linux has never been run on this machine, so no AppImage of the product exists from here, and none was ever started on a Linux desktop.
- The `x86_64` checksums were read from GitHub's listing of the two releases. The `x86_64` files were not downloaded or checked here; the first `x86_64` run of the script is their first test.
- Starting the image the normal way (mounted, without `--appimage-extract-and-run`).
- The application's own "restart" inside an AppImage.

### GitHub: `build.yml`

`build.yml` had not run since the rename when the packages changed. The tag `build-linux-windows-2` was pushed on 2026-11-13, on the commit that changed the packages, to start the Linux and Windows builds and the `draft-release` job. Its result was not known when this was written: this page does not say that those builds pass. The steps added to the workflow with the packages (the `file` package, the Inno Setup step, the new upload paths, `draft-release`) had never run before that tag. The macOS jobs were not started by it.

Expect to fix something on the first real run.
