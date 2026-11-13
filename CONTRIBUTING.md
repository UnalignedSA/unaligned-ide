# Contributing to Unaligned IDE

Unaligned IDE is a code editor made for coding with AI. It is a fork of the Zed editor: most of the code under `crates/` comes from that project, and the fork keeps its licences and its credit (see [`NOTICE`](NOTICE)).

The project is in pre-release. Changes are welcome; read this page first.

## Build it

[`README.md`](README.md), section "Build from source", has the commands for Linux, Windows and macOS. The Rust toolchain is pinned in `rust-toolchain.toml`.

Start a development build with `--user-data-dir <directory>`, so that its settings, data and logs stay in that directory.

## Where the documentation is

Everything written for contributors is in [`docs/unaligned/`](docs/unaligned/):

- [`CHARTER.md`](docs/unaligned/CHARTER.md): what the product is, its design language, the state of the fork and its rules. Read it before a first change.
- [`PLATFORM-CHECKS.md`](docs/unaligned/PLATFORM-CHECKS.md): type-checking for Linux and Windows from another system.
- [`SCREENSHOTS.md`](docs/unaligned/SCREENSHOTS.md): taking pictures of the application from the command line (macOS).
- [`NETWORK.md`](docs/unaligned/NETWORK.md): which hosts the application contacts, and how that is measured.
- [`PACKAGING.md`](docs/unaligned/PACKAGING.md): what can be packaged and how.

The rules for Rust and GPUI code are in [`.rules`](.rules) (`AGENTS.md` and `CLAUDE.md` are links to the same file).

The product's name, identifiers and addresses live in one file, `crates/branding/brand.env`. Use `branding::PRODUCT_NAME` and its siblings in code, never a literal name.

## Names

The application is the cargo package `ide` (`crates/ide`) and its binary is `unaligned-ide`. Actions are in the `app::` namespace, environment variables start with `IDE_`, a project's own settings folder is `.unaligned/`, and the app's internal links are `ide://`. These names replaced upstream's on 2026-11-13, through `script/unaligned/rename-upstream-names`; its header lists what it renames and what it leaves alone on purpose (licence texts, the extension API, Rust identifiers inside the crates).

- Do not give a new file, crate, action, variable or settings key upstream's name. `script/unaligned/check-branding` reports such a name in the text it scans; its header says what that is.
- To merge upstream: check its commit out on a temporary branch, run `script/unaligned/rename-upstream-names` there, commit, and merge that branch. Then `cargo check` regenerates `Cargo.lock`.

## Checks to run before sending a change

From the root of the repository:

| Command | What it checks | Needs |
| --- | --- | --- |
| `cargo fmt --all -- --check` | formatting | the pinned toolchain |
| `./script/clippy` | lints, with warnings as errors | the pinned toolchain |
| `cargo test -p <crate>` | the tests of the crates you changed | the pinned toolchain |
| `script/unaligned/check-branding` | no text a user can read or reach presents the product as its upstream | Python 3 |
| `script/unaligned/check-themes` | the bundled themes: required tokens, colours, contrast | Python 3 |
| `script/unaligned/check-linux` | the application type-checks for Linux | Docker |
| `script/unaligned/check-windows` | the application type-checks for Windows | Docker |
| `./script/check-keymaps` and `./script/check-todos` | keymap files and unfinished-code markers | git |

Each of the `script/unaligned/check-*` commands ends with a `PASS` or `FAIL` line. `check-linux` and `check-windows` accept `-p <crate>` to check less than the whole application; run them after any change to platform-specific code or to a crate's dependencies.

The workflow in [`.github/workflows/check.yml`](.github/workflows/check.yml) is set up to run on pull requests that come from forks; [`.github/workflows/README.md`](.github/workflows/README.md) says what it runs and what a green run does and does not prove.

## What makes a change easy to accept

- One subject per change.
- Tests for behaviour that can be tested.
- For a visible change, a screenshot. On macOS, `script/unaligned/shot` takes one from the command line (see `SCREENSHOTS.md`).
- Say what you ran and what you could not run. Linux and Windows are the release targets: say so if you could only try your change on another system.
- No secrets in the repository: no API keys in files, logs or screenshots.
- You must understand and be able to explain what you send, whatever tool wrote it.

## Licence of contributions

Unaligned IDE is licensed under the GNU General Public License, version 3 or later ([`LICENSE-GPL`](LICENSE-GPL)). GPUI and some support crates are under the Apache License 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE)). The `license` field of a crate's `Cargo.toml` says which one covers it.

A contribution is accepted under the licence of the crate it changes: GPL-3.0-or-later, or Apache-2.0 for the crates that say so. Only send code you have the right to contribute under that licence.

Do not remove or edit licence files, `NOTICE` or copyright lines. A new crate declares its licence in `Cargo.toml` and carries a `LICENSE-GPL` or `LICENSE-APACHE` link like the others (`script/check-licenses` verifies this).

## Bugs and questions

Bugs and questions go to the issue tracker of the source repository.

Conduct in the project's spaces is covered by [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).
