# Workflows

Three workflows live here: `check.yml` ("Check"), `build.yml` ("Build", test builds to download: see the end of this page) and `retry-macos.yml` ("Retry macOS", which starts the macOS jobs of a build again when GitHub gave them no machine: last section). There is no separate `release.yml`: since 2026-11-13 a version tag (`v0.1.0`) starts `build.yml` for every target and drafts the release (never run yet). Nothing else belongs in this directory.

Not run since the 2026-11-13 rename: `check.yml` (the same commands pass in the Docker checks on the development Mac: `check-linux` and `check-windows` for `-p ide` and `-p cli`). For `build.yml`, the tag `build-linux-windows-2` was pushed on 2026-11-13, after the rename, to start its Linux and Windows jobs; the result of that run was not known when this was written. The commands of both now name the package `ide` (`cargo check -p ide`), the Windows job reads its resources from `crates/ide/resources/windows`, and the three bundle scripts that `build.yml` runs stage the files under new names (`docs/unaligned/PACKAGING.md`, "File names inside the packages") and write new kinds of packages (an installer, disk images, an AppImage: see the end of this page).

**State: written on 2026-11-12 on a machine that cannot run GitHub Actions, and never executed.** The repository had no remote of its own at the time. The file was validated as far as that allows (see "How this was validated"); its first real run is the first push to the fork's own GitHub repository. Expect to fix something on that day.

## What runs

`check.yml` starts on pull requests coming from forks and by hand from the Actions tab ("Run workflow"). It no longer starts on every push (changed on 2026-11-12, before the first push): while the repository is private its two jobs are hours of machine time counted against the plan. The comment above `on:` in the file says what to put back when the repository is public. Tags do not start it.

Two jobs, on GitHub-hosted runners, each limited to 120 minutes:

| Job | Runner | Steps |
| --- | --- | --- |
| `linux` | `ubuntu-24.04` | Frees disk space (removes the preinstalled Android, .NET and Haskell toolchains), installs the system packages with `./script/linux`, installs the Rust toolchain named in `rust-toolchain.toml`, restores the cache. Then: `cargo fmt --all -- --check`, `./script/check-keymaps`, `./script/check-todos`, `python3 script/unaligned/check-themes`, a guard on the content of this directory, `cargo check -p ide --locked`, `cargo check -p cli -p remote_server --locked`, and the focused tests of `settings`, `theme`, `theme_settings`, `ui`, `workspace`, `title_bar`, `language_models`, `auto_update`. |
| `windows` | `windows-2022` | Installs the Rust toolchain, restores the cache. Then: `cargo check -p ide --locked`, `cargo check -p cli -p auto_update_helper -p explorer_command_injector --locked`, and the tests of `settings` and `workspace`. |

Once the toolchain is installed, every check step runs even when an earlier one failed, so a single run lists everything that is wrong. The job is red if any step failed.

It needs no secret, no paid runner and no account anywhere. Permissions are read-only (`contents: read`). The only actions used are `actions/checkout` and `Swatinem/rust-cache`, pinned to a commit.

## What a green run proves

- The whole application (`ide` crate) and the command-line tool type-check on real x86_64 Linux and Windows machines, as do the remote server on Linux and the two Windows helpers on Windows, with their build scripts running natively. On Windows that includes what the Docker checks on the development Mac cannot do: compiling the resources (icon, manifest, version information) and downloading ConPTY.
- The focused test suites compile, link and pass on Linux (eight crates) and on Windows (two crates).
- The source is formatted, the keymaps contain no macOS-only keys outside the macOS files, no unfinished-code marker is committed, the bundled themes are complete, and no workflow other than the fork's is present.

## What it does not prove

- No executable of the application is linked (`cargo check` stops before that), nothing is started, no window is opened. Rendering, fonts, window controls, key bindings and file dialogs on Linux and Windows are not looked at.
- No installer or archive is built by `check.yml` (packages are `build.yml`, below), and release-profile code (`cfg(not(debug_assertions))`, the Windows shader compilation) is not compiled.
- Only the crates listed above are tested, not the workspace. `cargo clippy` is not run (it is not clean today).
- Linux on aarch64 and Windows on arm64 are not covered.

## Expected on the first run

- **`Formatting` fails.** On 2026-11-12 `cargo fmt --all -- --check` reported 49 differences in 21 files. This is known and left for the hardening wave (wave 5); the other steps run anyway.
- **No test is skipped by name.** `SKIP_KNOWN_FAILURES` at the top of `check.yml` is empty: the three tests it listed were fixed. A test that must be skipped for a known reason goes there, and comes out in the same change that fixes it.
- **Other failures are possible and are the point.** These tests have only ever run on macOS, and the application has never been compiled natively on Windows.

## How long it takes (estimate, not measured)

First run, empty cache: about 60 to 90 minutes per job on the 4-core runners GitHub gives public repositories; the two jobs run side by side. This is extrapolated from the Docker type-checks on the development Mac (6 and a half minutes for `cargo check -p ide` with six fast cores) plus the full compilation the tests need. Later runs reuse the cached dependencies and should take about half of that. On the smaller runners of a private repository a first run may reach the 120-minute limit: raise `timeout-minutes` if it does.

Cost: free for a public repository. For a private one the minutes count against the plan, and Windows minutes count double.

## Disk

The Linux job deletes the preinstalled Android, .NET and Haskell toolchains before building (an estimated 15 to 20 GB). Debug information is turned off (`CARGO_PROFILE_DEV_DEBUG` and `CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG` set to 0; both, or cargo compiles several hundred crates twice). Both jobs print the free space at the end. The free space of the Windows runner could not be established beforehand: if that job stops with "There is not enough space on the disk", the first figures to read are those printed by its `Disk space` steps.

## Upstream's workflows are gone on purpose

Upstream keeps 46 generated workflows here (triage bots, deployments, release management). They need its paid runners and its credentials; 23 of them carry no owner guard and would start in any repository that has them. All of them were deleted, together with upstream's issue and pull-request templates, funding file, dependency-bot configuration and workflow-linter configurations.

- The generator that wrote them (`cargo xtask workflows`) was deleted too, on 2026-11-13.
- A merge from upstream reports those files as "deleted by us, modified by them": keep them deleted. A workflow upstream *added* since the last merge arrives without a conflict; the step `Only the fork's workflows` fails when any file other than `check.yml`, `build.yml`, `retry-macos.yml`, `release.yml` and this page is present. To add a workflow of your own, add its name to that step.
- Before a merge, upstream's commit goes through `script/unaligned/rename-upstream-names` (its header says how), so that both sides use the same names.
- Never define `IDE_MINIDUMP_ENDPOINT` or `IDE_CLIENT_CHECKSUM_SEED` in a workflow: a build that sees the first one sends crash reports again.

## Before the first push

- If the GitHub repository is created with the "Fork" button, Actions are off until they are enabled in the Actions tab. A repository created empty and pushed to has them on.
- The local `origin` points at upstream's repository (fetch only). Push to the fork's own remote, never to `origin`.

## Maintenance

- The two actions are pinned by commit, with the version as a comment: `actions/checkout` v5.0.1 (the commit upstream itself uses) and `Swatinem/rust-cache` v2.9.3. To update, replace the commit and the comment together; `git ls-remote --tags https://github.com/<owner>/<action>.git` gives the commit of a tag (for an annotated tag, the line ending in `^{}`).
- Runner images, checked on 2026-11-12 against GitHub's runner-images list: `ubuntu-24.04` and `windows-2022` are offered and not deprecated. The plan named `ubuntu-22.04` for the Linux job; that image is being retired (announcement 14254 of `actions/runner-images`: slower queues since 2026-09-17, deliberate failures on four days in March and April 2027, removal on 2027-04-17), so the job uses 24.04, for which `script/linux` has its own branch. GitHub keeps the two newest versions of each system: when a retirement is announced for one of these two, move to the next.
- The Rust version comes from `rust-toolchain.toml`; nothing here repeats it.

## How this was validated

On the development Mac, without GitHub: the YAML parses (Ruby's `YAML.load_file`, then a JSON round trip); every job, step, runner label, action commit, script path and crate name was checked by a script against the tree and against the published tags and the runner-image list; the shell of the guard step was run as GitHub runs it, with and without an unexpected file; the two profile variables were tried on a scratch crate with the pinned toolchain. `actionlint` was not installed and was not used. The commands themselves are the ones of the project's Docker checks (`script/unaligned/check-linux`, `check-windows`) and of its local test runs. Nothing of this replaces a run.

## `build.yml`: test builds to download

Written on 2026-11-12; its packages changed on 2026-11-13 (an installer for Windows, a disk image for macOS, an AppImage for Linux: no zip of the product's own any more). **No finished run is recorded here**: the tag `build-linux-windows-2` was pushed on 2026-11-13 to start the Linux and Windows builds on the new packaging, and its result was not known when this was written. The Windows script and the installer definition it compiles were written on a Mac and had never been run before that.

It builds the product for four targets and leaves each as an artifact on the run's page (kept 14 days). A run started by a tag also puts the files on a draft release. Nothing is signed and nothing is published.

| Job | Runner | What it runs | Artifact |
| --- | --- | --- | --- |
| `licenses` | `ubuntu-24.04` | `script/generate-licenses` (with `ALLOW_MISSING_LICENSES=1`) | `licenses` (kept 3 days), handed to the macOS and Windows jobs |
| `linux` | `ubuntu-24.04` | installs the `file` package (appimagetool needs it), then `script/unaligned/bundle-linux` | `unaligned-ide-linux-x86_64`: the `.AppImage`, the `.tar.gz` archive, `SHA256SUMS`, `install.sh` |
| `macos-arm` | `macos-15-intel` (Intel machine, Apple Silicon target) | `script/unaligned/bundle-mac aarch64-apple-darwin` | `unaligned-ide-macos-apple-silicon`: the `.dmg` |
| `macos-intel` | `macos-15-intel` | `script/unaligned/bundle-mac x86_64-apple-darwin` | `unaligned-ide-macos-intel`: the `.dmg` |
| `windows` | `windows-2022` | installs Inno Setup with Chocolatey only when `ISCC.exe` is not in `Program Files (x86)\Inno Setup 6`, then `script/unaligned/bundle-windows.ps1` | `unaligned-ide-windows-x86_64`: the installer `…-setup.exe` |
| `draft-release` | `ubuntu-24.04`, 30 minutes at most | only in a run started by a tag: downloads the artifacts above and attaches the `-setup.exe`, `.dmg` and `.AppImage` files, as they are, to a draft release named after the tag (`gh release create <tag> --draft --verify-tag`). For a version tag it first builds `update-manifest` (`crates/update_manifest`), writes `update.json` from the installers, signs it with the repository secret `UPDATE_SIGNING_KEY` and attaches `update.json` and `update.json.sig` too: they are what lets an installed copy update itself (`docs/unaligned/UPDATES.md`). Without the secret: a warning, `update.json` alone, and installed copies ignore the release. It refuses to replace the files of a version that is already published | none: the files are on the draft release |

About `draft-release`:

- The secret `UPDATE_SIGNING_KEY` is given to the step "Sign the list" and to nothing else: no build job, no other step. That step runs the `update-manifest` program built by the step before it, and no build or download of its own. The job only runs for a tag, so only someone who can push a tag to the repository can make it sign. After signing, the step checks the signature against `UPDATE_PUBLIC_KEYS` of the tagged commit and the files against the manifest: a key that does not match the brand file fails the run instead of producing a release no installed copy would accept.
- A run for a version tag starts by comparing the tag with the version in `crates/ide/Cargo.toml` (first step of the `licenses` and `linux` jobs) and fails when they differ.

- It is the only job that can write to the repository: `permissions: contents: write` on that job, for `gh release create`. The workflow's own permission stays `contents: read`, and the only secret used is the run's own `GITHUB_TOKEN`.
- It waits for the four build jobs and runs even when one of them failed or was not asked for: whatever was built is attached. It fails when no build produced a file. A cancelled run makes no release.
- The release is titled "Test build <tag>", or "Unaligned IDE <version>" for a version tag (`v0.1.0`). It is always a draft: publishing it is done by hand on the "Releases" page, and the README's download links (`releases/latest/download/…`) only see a published release that is not marked as a pre-release. When a release named after the tag exists already, the files are uploaded to it and replace those of the same name.
- A draft is seen only by people who can write to the repository. It is published by hand or not at all.
- The archive, `SHA256SUMS` and `install.sh` are not attached: they stay in the Linux artifact.
- A run started by hand makes no release. It only leaves artifacts.

How to start it:

- By hand: Actions tab, "Build", "Run workflow". The field takes `all` or any of `linux windows mac-arm mac-intel`.
- By a version tag, `git tag v0.1.0 && git push <remote> v0.1.0` (the version of `crates/ide/Cargo.toml`): every target is built. Added on 2026-11-13 and never run yet.
- By a tag whose name starts with `build-`; the words in the name choose the targets: `git tag build-all-1 && git push <remote> build-all-1`, `build-linux-2`, `build-windows-mac-arm-3`. These tags are only triggers. Deleting such a tag afterwards does not delete its draft release: delete the draft on the "Releases" page too.

Cost, for a private repository (rates read on GitHub's pricing page on 2026-11-12: Linux $0.006, Windows $0.010, macOS $0.062 per minute; durations are estimates, nothing was measured): the macOS jobs are by far the most expensive, about ten times a Linux minute. One full run can use more than a free plan's monthly minutes. A public repository pays nothing for these machines and gets larger ones for Linux and Windows. `draft-release` adds a few Linux minutes to a run started by a tag: it downloads and uploads files and compiles nothing. `script/unaligned/bundle-mac --runtime-shaders` builds the macOS application and its disk image on the development Mac for free.

Where the files are:

- A run started by a tag: the "Releases" page of the repository, on the draft named after the tag. The `-setup.exe`, the `.dmg` files and the `.AppImage` are there as they are: nothing to unzip.
- A run started by hand: the run's page, under "Artifacts". GitHub wraps each artifact in a zip: unzip it once to get the same files. (A run started by a tag has these artifacts too.)

Trying the downloads:

- macOS: open the `.dmg`, drag the application onto the Applications link, then run `xattr -dr com.apple.quarantine "/Applications/Unaligned IDE.app"` before the first start: the application is not signed by an Apple developer identity (only by the product's own certificate, for the keychain: `docs/unaligned/PACKAGING.md`), and macOS refuses a downloaded one otherwise.
- Windows: start `unaligned-ide-windows-x86_64-setup.exe`; SmartScreen asks once ("More info", "Run anyway"). The installer asks for no administrator rights and adds a Start-menu entry.
- Linux: `chmod +x unaligned-ide-linux-x86_64.AppImage`, then start it. The archive and `install.sh` are only in the run's artifact: unpack the archive and run `unaligned-ide.app/bin/unaligned-ide`, or use `install.sh`.

Known gaps of these builds: licence list not checked for completeness; no bundled `git` on macOS (the system's is used); the two macOS jobs run on GitHub's Intel machines (`macos-15-intel`) since 2026-11-13, when the Apple Silicon ones (`macos-15`) left the jobs waiting until GitHub cancelled them, three times in a row: a build on the Intel machines had never been run when this was written, and the Apple Silicon build cannot be started on them, so that job does not ask the packaged command its version; the Windows installer has never been compiled or run, and the AppImage was only ever packed from a stand-in application (`docs/unaligned/PACKAGING.md`, "Executed and not executed").

## `retry-macos.yml`: when GitHub has no Mac

Since 2026-11-02 GitHub's macOS machines leave jobs waiting and cancel them after fifteen minutes, on and off, with the annotation "The job was not acquired by Runner of type hosted even after multiple attempts" (`actions/runner` issue 4833, community discussions 211885 and 212069; other projects report it on the Apple Silicon and on the Intel machines alike, and GitHub's status page showed nothing on 2026-11-13). The two macOS jobs of the 0.1.1 build were cancelled that way again and again that day, on `macos-15` and then on `macos-15-intel`. Nothing in the build is wrong then; the only cure is to ask again.

"Retry macOS" does the asking. It starts each time a "Build" run ends, and does something only when all of this holds:

- the run ended as failed (a run somebody cancelled is left alone);
- at least one job whose name starts with "macOS" was cancelled before its first step;
- no job failed for a reason of its own (a real build error is never retried);
- the run is below its twelfth attempt.

It then waits twenty minutes and runs `gh run rerun <run> --failed`: the macOS jobs start again, and after them `draft-release`, which writes and signs `update.json` again from all the files the run has. The Linux and Windows files of the first attempt are kept. Its only permission is `actions: write`; it reads no secret and checks out no code.

To stop it: cancel the "Retry macOS" run while it waits, or disable the workflow (Actions tab, "Retry macOS", "…", "Disable workflow").

Not run when this was written: the condition was tried by hand on the job lists of the three "Build" runs of 2026-11-13 (two with macOS jobs that never started, one that succeeded), the rest never. In particular, whether a re-run started by this workflow starts this workflow again when it ends was not observed; if it does not, one "Re-run failed jobs" by hand gives one more attempt each time.
