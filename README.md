# Unaligned IDE

A free, open-source code editor made for coding with AI. Fast (Rust, GPU-drawn), laid out like VS Code, with one chat panel and one screen to connect the model you want: an API key, a subscription you already pay for, a local model, or several at once.

It is a fork of the [Zed](https://github.com/zed-industries/zed) editor. [Unaligned](https://unaligned.pw), a privacy-first, pay-as-you-go AI API, is the featured provider and never the only one.

> **Pre-release.** There is no public download yet: you build it from source, or with the packaging scripts below. "Unaligned IDE" is a working name.

## What you get

**AI that is simple to use**

- One chat panel with two modes: **Agent** (reads and edits your files, runs commands) and **Ask** (answers only).
- `Ctrl+L` opens the chat with your selection, `Ctrl+K` edits the code under the cursor, `Ctrl+I` starts a new chat (`Cmd` on macOS).
- Every edit the agent makes is shown for review: keep or undo, per file or all at once.
- The chat says what is going on in plain words: not connected, key refused, balance too low, rate limited, server down. A refused prompt can be edited and sent again; "Allow All" stops the questions before each tool.
- With Unaligned: the balance in the status bar, refreshed after every request, and the cost of each chat.

**Any model, your way**

- **Unaligned**: paste a key, pick a model from the live list with its prices.
- **Subscriptions**: ChatGPT, GitHub Copilot, SuperGrok, and the vendors' own agents (Claude Code, Codex, Gemini CLI) inside the same chat.
- **Your own keys and local models**: OpenAI, Anthropic, Google, Mistral, DeepSeek, xAI, OpenRouter, Amazon Bedrock, Ollama, LM Studio, llama.cpp, or any OpenAI-compatible address.

**An editor you already know**

- VS Code keys and layout by default: activity bar, explorer, tabs, terminal at the bottom.
- Every panel, the chat included, moves by drag and drop to the left, right or bottom. Layout presets: Classic, AI-First, Zen.
- Five built-in themes (Unaligned, Light, Dusk, Midnight, Contrast) plus One, Ayu and Gruvbox. Corners and accent colour are settings.
- Underneath, Zed's editor: language servers, git, terminal, debugger, search, extensions.

**Private by default**

- No account, no telemetry, no crash reports. Nothing is uploaded but what you send to the model you connected.

## Status

| Platform | State |
| --- | --- |
| macOS, Apple Silicon | Builds and runs (development and release builds). |
| macOS, Intel | Builds. Not yet tried on an Intel Mac. |
| Linux x86_64 | Type-checks. First real builds are being made on GitHub's machines. |
| Windows x86_64 | Type-checks. First real builds are being made on GitHub's machines. The installer has not been tried on Windows yet. |

After the rename of 2026-11-13 (package `ide`, binary `unaligned-ide`, new file names inside the packages) the development build, the tests and the Linux and Windows type-checks were run again on a Mac. The Linux and Windows packages have not been built with the new names yet.

Nothing is signed: macOS and Windows ask before the first start of a downloaded build (see [Packaged builds](#packaged-builds)).

Known limits:

- There is no user manual yet. Help → Documentation opens the documentation of the Unaligned API.
- Language extensions come from Zed's extension registry and are built against its extension API.
- The name "Zed" is still shown where it states a fact, such as the credit in About and the line of the Extensions page that names the registry.
- GitHub Copilot and the ChatGPT sign-in are still told that the editor is Zed: those identities are registered by upstream with the two vendors.
- The sign-ins of the subscriptions are upstream's code and were not tried one by one here.
- Remote development (SSH, WSL, containers) needs a server build this project does not publish yet.
- A project's own settings, tasks and debug scenarios are read from `.unaligned/`. A `.zed/` folder is not read.

## Build from source

You need [rustup](https://www.rust-lang.org/tools/install); the toolchain is pinned in `rust-toolchain.toml`. The first build compiles the whole application and takes a long time.

Start a development build with `--user-data-dir <directory>`: its settings, data and logs then stay in that directory.

**Linux**

```sh
script/linux            # installs the system libraries
cargo build -p ide
./target/debug/unaligned-ide --user-data-dir /tmp/unaligned-ide-dev path/to/a/folder
```

**Windows** (Visual Studio build tools with MSVC, and CMake; from a developer shell)

```powershell
cargo build -p ide
.\target\debug\unaligned-ide.exe --user-data-dir $env:TEMP\unaligned-ide-dev path\to\a\folder
```

**macOS**

```sh
cargo build -p ide
./target/debug/unaligned-ide --user-data-dir /tmp/unaligned-ide-dev path/to/a/folder
```

On a Mac with the Command Line Tools but without Xcode, add `--features gpui_platform/runtime_shaders` to every command that builds the `ide` crate.

The application is the cargo package `ide`; its binary is `unaligned-ide`.

### Packaged builds

| Platform | Command | Result |
| --- | --- | --- |
| Linux | `script/unaligned/bundle-linux` | `target/release/unaligned-ide-linux-<arch>.AppImage` (one file that runs where it is), and a `.tar.gz` with an `install.sh` |
| macOS | `script/unaligned/bundle-mac [aarch64-apple-darwin \| x86_64-apple-darwin]` | `target/release/unaligned-ide-macos-<arch>.dmg` (the application and a link to Applications) |
| Windows | `script/unaligned/bundle-windows.ps1` (needs Inno Setup 6) | `target\release\unaligned-ide-windows-x86_64-setup.exe` (an installer for the current user) |

The **Build** workflow (`.github/workflows/build.yml`) runs the same scripts on GitHub's machines. It starts by pushing a tag such as `build-all-1` or `build-linux-windows-2`, and then puts the files on a draft release named after the tag; started by hand from the Actions tab, it leaves them as artifacts of the run.

First start of a downloaded build (nothing is signed yet, so each system asks once):

- macOS: open the `.dmg`, drag the application to Applications, run `xattr -dr com.apple.quarantine "/Applications/Unaligned IDE.app"`, then open it.
- Windows: start the installer; SmartScreen asks once ("More info", "Run anyway").
- Linux: `chmod +x unaligned-ide-linux-x86_64.AppImage`, then start it.

Details and rules for a real release: [`docs/unaligned/PACKAGING.md`](docs/unaligned/PACKAGING.md).

## Connect a model

Open the chat (`Ctrl+L`). Until something is connected it is a connection screen with three groups: Unaligned, subscriptions, your own keys and local models. For Unaligned, create a key on [unaligned.pw](https://unaligned.pw/dashboard/keys) and paste it: the key is checked, the balance shows, and the model list loads.

More connections can be added later from the chat's "…" menu, "Connections…".

## Privacy

What the application contacts, and when:

- the AI provider you connected, when its key is checked and when you send a message;
- `localhost`, to look for local models (Ollama, LM Studio, llama.cpp);
- the public registry of external agents, when you set one up;
- Zed's extension registry, for language extensions;
- GitHub, npm and nodejs.org, when a language server has to be downloaded for a file you opened.

This is measured, not only stated: [`docs/unaligned/NETWORK.md`](docs/unaligned/NETWORK.md) lists the hosts seen in each scenario, and `script/unaligned/netcheck` runs the measurement again on your own build.

## For contributors

- [`CONTRIBUTING.md`](CONTRIBUTING.md): the checks to run before sending a change.
- [`docs/unaligned/CHARTER.md`](docs/unaligned/CHARTER.md): the brief, the design language, the state of the fork and its rules.
- [`docs/unaligned/`](docs/unaligned/): the tooling pages (screenshots, platform checks, network, packaging).
- `crates/branding/brand.env`: the product's name, identifiers and addresses, in one file.
- `script/unaligned/`: the project's own tooling (`check-linux`, `check-windows`, `check-themes`, `check-branding`, `netcheck`, `shot`).
- `script/unaligned/rename-upstream-names`: gives the names inherited from upstream (crates, binary, actions, variables, link schemes, the project folder) the fork's own. To merge upstream, run it on upstream's commit first; its header says how.
- [`.github/workflows/README.md`](.github/workflows/README.md): what the two workflows do and cost.

## Licence and credit

Unaligned IDE is free software under the **GNU General Public License, version 3 or later** ([`LICENSE-GPL`](LICENSE-GPL)), with no warranty. GPUI and some support crates are under the **Apache License 2.0** ([`LICENSE-APACHE`](LICENSE-APACHE)); a crate's `Cargo.toml` says which licence covers it, and [`NOTICE`](NOTICE) lists the files this fork changed in Apache-2.0 crates.

It is based on [Zed](https://github.com/zed-industries/zed), © Zed Industries, Inc., modified by the Unaligned IDE contributors since 2026. "Zed" is used here only to say where the code comes from: Unaligned IDE is a separate project and is not Zed.

The bundled fonts (Onest, JetBrains Mono, Unbounded, IBM Plex Sans, Lilex) are under the SIL Open Font License 1.1; the bundled themes are under the MIT License; the icons include the Lucide set (ISC). The licences of the Rust dependencies are collected when the application is packaged and shown on its licences page.
