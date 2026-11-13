# Updates

How an installed copy learns that a newer version exists, how it installs it, and how a release is made installable. Code: `crates/auto_update` (the check and the installation), `crates/update_manifest` (the signed list of a release's files, and the `update-manifest` program), the three steps before "Draft release" in `.github/workflows/build.yml`.

## What the application does

1. **The check.** Once a day, and when the user picks Help → Check for Updates, the application asks `https://api.github.com/repos/<RELEASES_REPOSITORY>/releases` for the latest published release (drafts and pre-releases do not count). It is an anonymous request, with no identifier. The setting `"auto_update": false` turns the daily check off; the menu entry still works. A development build never checks by itself.
2. **Is it ours?** When that release's version is higher than the running one, the application downloads two small files of the release from `https://github.com/<RELEASES_REPOSITORY>/releases/download/<tag>/`:
   - `update.json`, the manifest: the product, the version, and for each installer its name, its size and its SHA-256;
   - `update.json.sig`, an Ed25519 signature of the manifest.

   The release is announced only when the manifest is signed with the product's update key and is this product's and that version's. **A release without a valid signature is ignored**: no button, no link. Help → Check for Updates says why.
3. **The announcement.** The title bar shows **Update Available**. No installer has been downloaded at this point.
4. **The installation, on a click.** The manifest and its signature are downloaded and checked again, then the installer of the platform, which must have the size and the SHA-256 the manifest gives. The new version is put in place the way the copy was installed:

   | Installed as | What happens |
   | --- | --- |
   | macOS application | The disk image is opened without showing it, the new application is copied beside the installed one, and the file system exchanges the two in one step. Until that step the installed application is untouched. |
   | Linux AppImage | The new file is written beside the running one, flushed to disk, and takes its name in one rename. |
   | Windows, by the installer | The downloaded installer is kept; it runs when the application restarts (`/SILENT /NOCANCEL /update=true`), replaces the files and starts the new version. |
5. **The restart.** The button becomes **Restart to Update**. On macOS and Linux the new version is already in place, so quitting and starting again does the same. On Windows the installer runs at the next restart the application does itself (that button, Help → Check for Updates → Restart, or the restart command); after a plain quit, the update is simply offered again.

## What is checked, and what an attacker is left with

- The manifest must be signed by one of the public keys compiled into the application (`UPDATE_PUBLIC_KEYS` in `crates/branding/brand.env`). The addresses of the files are built from the repository's name and the tag; nothing in GitHub's answer can point a download elsewhere.
- The manifest must name this product and exactly the version that was announced, which is higher than the running one. An old release, however genuine its signature, cannot be served under a newer number, and nothing older than the running version is ever installed.
- The installer must have exactly the size and the SHA-256 the manifest gives. A download that grows past the signed size is stopped at once, and every download has a time limit, so a silent server cannot hold the updater for ever.
- When one of these checks fails at the click (the release changed since the check), nothing is installed and the release is no longer offered: no retry button, no link to its page.

So someone who can publish or alter releases in the repository, but does not hold the private key, cannot make an installed copy install anything, and cannot even get a release of theirs announced. What they can still do:

- make genuine updates fail or disappear, by breaking the files of a release (a denial, visible in Help → Check for Updates);
- keep a copy on an older genuine release that is still newer than the running one, by hiding the latest (a freeze, not a downgrade);
- change the files a person downloads **by hand** from the release page. The builds are not signed for the operating system (no Windows certificate, no Apple notarisation), so nothing checks a manual download: this is true of the first installation too.

A build without a key (`UPDATE_PUBLIC_KEYS` empty, as in a fork that has not made one) cannot check anything: it announces the latest release as it is, with a link to its page, and installs nothing.

## When a copy announces without installing

The release is signed, and the button opens its page instead of installing, when:

- the signed manifest lists no installer for this platform;
- macOS: the application is not a bundle (a build from source), its folder cannot be written to (a mounted disk image, an account without the right), or macOS runs it from a read-only copy because it was started straight from the downloads folder. Move it to Applications first;
- Linux: the application is not running from an AppImage (the tarball, a build from source), or the AppImage's folder cannot be written to;
- Windows: the folder of the executable has no `unins000.exe`, so it was not made by the installer.

A packager who sets `IDE_UPDATE_EXPLANATION` (at build or at run time) turns the check off altogether; "Check for Updates" then shows that text.

## Making a release

Everything runs on GitHub (`.github/workflows/build.yml`):

1. Set the new version in `crates/ide/Cargo.toml` (and the badge at the top of `README.md`), build once locally so that `Cargo.lock` follows, commit.
2. Tag that commit `v<version>` and push the branch and the tag. The workflow refuses a tag that is not the version of `crates/ide/Cargo.toml`: an application whose version is lower than its own release's would announce that release to itself for ever.
3. The workflow builds the four installers. Its last job then writes `update.json` from them, signs it with the repository secret `UPDATE_SIGNING_KEY`, checks the signature against the public key of that very commit and the files against the manifest, and attaches everything to a draft release.
4. Publish the draft, as the latest release. Installed copies see it within a day.

Rules:

- **Never rebuild a published version.** A second signed manifest for the same number would let someone serve either build. The workflow refuses to replace the files of a published release: fix forward with a new version.
- **Without the secret** the release is built all the same, with a warning and with `update.json` alone. Installed copies ignore such a release, unless the signature is made by hand and added to the draft before publishing (see "The key").

## The key

- Created once with `cargo run -p update_manifest -- new-key <file>` (the file is created with owner-only permissions and never replaced). The program prints the public key, which goes into `UPDATE_PUBLIC_KEYS`.
- The private key is the content of that file. It is stored as the repository secret `UPDATE_SIGNING_KEY` (Settings → Secrets and variables → Actions). The workflow gives it to one step, "Sign the list", which runs the already built `update-manifest sign` and nothing else: no build, no download, no other job sees it. It must never be committed.
- **Whoever holds the private key can sign an update that every installed copy accepts.** With the key in a GitHub secret, that includes anyone who can change the workflows of the repository or push a tag to it: protect the account (two-factor authentication, no shared tokens with write access). To keep the key off GitHub altogether, do not set the secret, and sign by hand before publishing: download `update.json` from the draft, run `update-manifest sign` on it with the key file, and add the `update.json.sig` it writes to the draft.
- **Losing the key** cuts installed copies off from updates: they ignore every later release, and a version with a new public key has to be installed by hand once. Keep a copy of the key file somewhere safe.
- **Changing the key**: `UPDATE_PUBLIC_KEYS` takes several keys separated by commas. Release a version that trusts both the old and the new key, signed with the old one; sign the following releases with the new one; drop the old key later.

## The `update-manifest` program

```sh
cargo run -p update_manifest -- new-key <private-key-file>
cargo run -p update_manifest -- public-key <private-key-file>
cargo run -p update_manifest -- make --product unaligned-ide --version 0.1.2 --out update.json <installer>...
cargo run -p update_manifest -- sign update.json --key-file <private-key-file>     # or the variable UPDATE_SIGNING_KEY
cargo run -p update_manifest -- verify update.json update.json.sig --public-keys <key>[,<key>] [--version 0.1.2] [--files <directory>]
```

The signature covers the exact bytes of `update.json`: the file must not be reformatted after signing. Never sign a manifest of test files with the real key under a version that could be released: a signed manifest is valid for every installed copy below its version.

## Trying an update without publishing one

A development build (and only a development build: the code is not compiled into a release) reads its releases from a stand-in for GitHub when two variables are set:

- `IDE_UPDATE_TEST_ORIGIN`, for example `http://127.0.0.1:18473`. The stand-in serves GitHub's JSON list of releases at `<origin>/repos/<RELEASES_REPOSITORY>/releases` and the files at `<origin>/<RELEASES_REPOSITORY>/releases/download/<tag>/<name>`.
- `IDE_UPDATE_TEST_PUBLIC_KEY`, the public key of a key made for the test with `update-manifest new-key`.

Add `IDE_RELEASE_CHANNEL=stable`, because a development build does not check for updates otherwise, and start the binary from inside an application bundle (macOS) for the installation to be possible.

## What was verified, and what was not

Verified on the development Mac (Apple Silicon), on 2026-11-13:

- The tests of `update_manifest` and `auto_update`. Not announced: a release signed with another key, a genuine older release under a newer number, a release without a manifest or without a signature. Not installed, and withdrawn: a release whose manifest, installer or version is swapped between the check and the click. A file longer than signed is refused; a failed download keeps the release on offer. A check requests the list, the manifest and the signature, never an installer.
- A real replacement of a macOS bundle from a disk image (`hdiutil`, `ditto`, the exchange), in a test and in the running application: a development build inside a bundle, a local stand-in for GitHub serving a signed "9.9.9": **Update Available**, a click, the downloads, the bundle replaced in about ten seconds, the image detached and the download removed, **Restart to Update**, and the restart starting a new process from the replaced bundle.
- The replacement of an AppImage file (permissions kept, nothing left behind, an unwritable folder leaves the old file), in a test on macOS.
- The workflow's three signing steps, run by hand on local files, with and without the key.
- A second reading of the code by an independent reviewer, whose findings are in the points above (releases the key did not sign are not announced, the key is given to the signing command alone, the exchange in one step, the time limit on downloads).

Not verified:

- **Windows, end to end.** The installer's `/update=true` line (`crates/ide/resources/windows/installer.iss`), `/NOCANCEL` and the restart into the installer were never run: the code is type-checked for Windows only. The first real test is the first update between two published versions.
- **Linux, end to end**: the same; the detection relies on the `APPIMAGE` and `APPDIR` variables the AppImage runtime sets.
- **macOS from `/Applications`, started from the Finder.** The test ran the bundle from a terminal, in the working directory. macOS can restrict an application that modifies application bundles ("App Management"); if it refuses the exchange, the update fails cleanly and the release's page is offered.
- The check against the real GitHub API with a real newer release, and the workflow steps on GitHub's machines.
