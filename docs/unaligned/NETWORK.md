# Network audit: which hosts does the IDE contact?

Product principle 4 says nothing goes to Zed's servers without an explicit, visible reason. `script/unaligned/netcheck` measures it instead of trusting a code reading: it runs one private instance of the app behind a logging proxy, through a scripted session, and prints the hosts that were contacted and a verdict.

```sh
script/unaligned/netcheck                                   # scenario startup: table, then PASS or FAIL
script/unaligned/netcheck --scenario all --expect today     # the three scenarios against today's baseline
script/unaligned/netcheck --scenario startup --expect target   # what the plan must reach
script/unaligned/netcheck --scenario startup --expect target --deny cdn.agentclientprotocol.com
script/unaligned/netcheck --scenario startup --expect today -- --open src/store.rs --open web/app.ts
```

A scenario takes 25 to 35 seconds. Nothing is installed, no system setting is touched, nothing is decrypted, and only the instance started by the script is observed: the proxy listens on a free port of 127.0.0.1 that only that instance is told about.

## Before the first run

- Build the app: `cargo build -p ide --features gpui_platform/runtime_shaders`. The audit runs `target/debug/unaligned-ide`.
- No variable ending in `API_KEY` in the environment: `netcheck` refuses to start (it prints the names, never the values). Run it with `env -u NAME script/unaligned/netcheck …`.
- The screen may be locked. No picture is taken unless `--picture` is given.

## Scenarios

| Scenario | What the instance does |
| --- | --- |
| `startup` | Opens the sample project (`.work/sample/skiff`, see `SCREENSHOTS.md`), no file, and is left alone for 20 s. |
| `extensions` | The same, then the action `app::Extensions` (the Extensions page), then 10 s. |
| `chat` | Scene `chat-reply` of `scenes.json` after the same 20 s: the sample project with `src/store.rs` open, the stand-in API (`shot --fake-api`, placeholder key), then the scene's steps (open the chat panel, type a message, send it), then 10 s. It is the scene `shot-all` keeps working, so the audit follows it when the chat changes. |
| `all` | The three, one after the other. |

## Options

| Option | Meaning |
| --- | --- |
| `--scenario NAME` | `startup` (default), `extensions`, `chat`, `all`. |
| `--expect today` / `target` | A named list of allowed hosts, see Lists. Without it only `127.0.0.1` and `localhost` are allowed. |
| `--allow HOST,…` | More allowed hosts. `*.example.com` matches subdomains. Repeatable. |
| `--deny HOST,…` | Hosts that fail even when a list allows them. Repeatable. |
| `--startup-wait S` | How long the instance is left alone after launch (default 20). |
| `--picture` | Also capture the window at the end (`.work/shots/netcheck-<scenario>.png`). Needs an unlocked screen. |
| `--keep` | Keep the instance's directory `.work/run/netcheck-<scenario>` (data, project copy, downloads). It can be several hundred megabytes when language servers were installed. |
| `--binary PATH` | Another build of the app. Never the offscreen one, see What the instance is. |
| `-- OPTIONS` | Everything after `--` is passed to `script/unaligned/shot`: `--open FILE` (a file of the sample, repeatable), `--env KEY=VALUE`, `--actions …` (replaces the scenario's steps), `--project DIR`. A `--settings JSON` given here is merged with the audit's own, and the proxy always stays. |

## Reading the output

```
== startup (expect target) ==
host                                        connections        first seen  last active  sent     received  through                              allowed
localhost                                   5 (5 unreachable)  0.6 s       0.9 s        0 B      0 B       HTTP :8080, HTTP :1234, HTTP :11434  yes
cdn.agentclientprotocol.com                 45                 0.6 s       16.5 s       80.3 kB  396.3 kB  CONNECT :443                         yes
api.zed.dev                                 2                  0.9 s       1.3 s        3.7 kB   10.4 kB   CONNECT :443                         NO
zed-extensions.nyc3.digitaloceanspaces.com  1                  1.4 s       3.1 s        2.2 kB   128.1 kB  CONNECT :443                         NO
53 connections to 4 hosts in 22 s; log: .work/netcheck/startup/proxy.log
settings of the instance that are not the product's defaults: agent.notify_when_agent_waiting, cursor_blink, proxy, …
FAIL startup: api.zed.dev, zed-extensions.nyc3.digitaloceanspaces.com
```

- **connections**: connections the instance asked the proxy for, not requests. The app keeps connections open and reuses them, so several requests to one host often show as one or two connections. "unreachable" counts those the proxy could not open (nothing listens there).
- **first seen**, **last active**: seconds after the app was started. A request sent later on a connection opened at startup shows only in "last active" and in the volumes: in `extensions` today, `api.zed.dev` is first seen at 0.7 s and last active at 22 s, with 386 kB received, which is the list of the Extensions page. The closing of an idle connection also counts as activity.
- **sent**, **received**: bytes that went to the host and came back, encryption included. For HTTPS this is all that can be known besides the host and the port.
- **through**: `CONNECT :443` is an encrypted tunnel; `HTTP :port` is a plain request (only local addresses use it today).
- **allowed**: `yes`, `NO` (in no list), `NO (denied)`, `NO (before the steps)` (a host the list allows only once the scenario's steps have begun, seen during the startup wait).
- The line about settings names every setting of the instance that differs from `assets/settings/default.json`, so a reader can judge how far the session is from a real first launch.
- **Verdict**, one line per scenario and, for `all`, one for the whole run: `PASS`, `FAIL: hosts`, or `ERROR: reason` when the session did not run to its end (the app crashed, a step named an unknown action, or in `chat` the message never reached the stand-in API through the proxy). Exit status 0, 1, 2.

Files of a run, all under `.work/` (git-ignored), replaced at each run: `.work/netcheck/<scenario>/proxy.log` (the proxy's lines), `app/*.log` (the app's own logs, where download messages name what was fetched), `settings.json` (what the instance ran with), `shot.log`.

## Lists

| Host | `today` | `target` |
| --- | --- | --- |
| `127.0.0.1`, `localhost` | allowed (built in) | allowed (built in) |
| `api.zed.dev`, `zed-extensions.nyc3.digitaloceanspaces.com` | allowed | only in `extensions`, and only once the Extensions page has been opened |
| `cdn.agentclientprotocol.com` | allowed | allowed (use `--deny cdn.agentclientprotocol.com` once W4-09 has made the fetch lazy) |
| `registry.npmjs.org`, `nodejs.org`, `github.com`, `api.github.com`, `objects.githubusercontent.com`, `release-assets.githubusercontent.com`, `codeload.github.com` | allowed | allowed |

Any other host fails, in particular `zed.dev`, `cloud.zed.dev` and every other `*.zed.dev`. The lists are at the top of `script/unaligned/netcheck` (`THIRD_PARTY`, `REGISTRY_HOSTS`, `EXPECT`).

## What the instance is

`netcheck` starts the app through `script/unaligned/shot`, so the instance has the layout described in `SCREENSHOTS.md`: its own data directory and home directory under `.work/run/netcheck-<scenario>`, a copy of the sample project, no variable that looks like a secret. On top of that:

- **The product's defaults where they matter for the network.** `shot` turns language servers off and disables the automatic install of the `html` extension for its pictures. `netcheck` reads `enable_language_server` and `auto_install_extensions` from `assets/settings/default.json` and passes those values, so a change of the defaults changes the audit.
- **The normal binary, always.** With a locked screen `shot` would use `.work/bin/ide-offscreen`. That build has test support compiled in and refuses to look for language server binaries ("language server binary lookup … is disabled in tests"), so it contacts fewer hosts than the product. `netcheck` passes `--binary target/debug/unaligned-ide --no-capture`: `shot` then runs the steps and takes no picture, with any screen. `script/unaligned/build-offscreen` puts a test-support build in `target/debug/unaligned-ide` for a few minutes while it works: `netcheck` recognises it (the binary is byte for byte `.work/bin/ide-offscreen`, or the instance's log says a language server lookup "is disabled in tests") and ends in `ERROR`; run it again afterwards. The check also applies with `--picture`. It misses one case: a test-support build that is not the one in `.work/bin` and that never looks for a language server.
- **The proxy**: setting `"proxy": "http://127.0.0.1:<port>"` for the app's HTTP client, and `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` (both cases) for child processes. `NO_PROXY` is emptied so that local addresses go through the proxy too.
- **`RUSTUP_AUTO_INSTALL=0`** (`shot` now sets it for every instance; `netcheck` still passes it explicitly)**.** The instance's home directory is empty and its project sits under this repository's `rust-toolchain.toml`: a `rustup` found on the `PATH` would download that whole toolchain from `static.rust-lang.org` the moment a Rust file is opened (measured: 12 connections in 10 s). No real installation does that, so it is switched off; the app then behaves as on a machine without rust-analyzer and downloads it from GitHub.
- Still different from a real first launch (the output lists them): the project is trusted (`session.trust_all_worktrees`: a real first open of a folder shows the "Unrecognized Project" prompt first, scene `restricted-mode`), the cursor does not blink, the agent's notification window is off, the terminal uses a plain shell.

## What it sees, and what it does not

Seen:
- Every request of the app's own HTTP client (extension registry, agent registry, model lists, language model providers, downloads of language servers and of Node, update checks): the client is built once with the `proxy` setting (`crates/ide/src/main.rs`, `ReqwestClient::proxy_and_user_agent`), and local addresses are included.
- Child processes that honour the proxy variables: npm (language servers, Prettier), git over HTTPS, curl, cargo, rustup.

Not seen:
- Programs that ignore the proxy variables. A language server or a debug adapter that opens its own connections (to fetch a schema, check for an update, send its own telemetry) is invisible unless it reads `HTTPS_PROXY`.
- DNS. Names are resolved by the proxy for the traffic it carries; lookups made by any other code are not observed.
- Anything that is not HTTP or a `CONNECT` tunnel: SSH (remote development, git over SSH), UDP (calls), local sockets and pipes.
- Links opened in the browser (documentation, sign-in pages): the browser makes those requests.
- What is inside an HTTPS connection: paths, headers and bodies. The app's log (`app/*.log`) often names the URL of a download.
- Other platforms. This is the macOS debug build; code under `cfg(target_os = "linux" | "windows")` is not run.

Limits of the scenarios:
- A debug build is on the `dev` release channel, where the update check never polls. To audit it, pass `-- --env IDE_RELEASE_CHANNEL=stable` (see W2-02).
- The instance has no stored credentials, so nothing that follows a sign-in is exercised. That check was run once by hand (the logging proxy, `shot --no-capture`, a planted `development_credentials` entry for `https://zed.dev`, then `client::SignIn`, `client::Reconnect`, `client::SignOut`): no `zed.dev`, no `cloud.zed.dev`, the log says "Accounts are not part of Unaligned IDE" twice, the planted file is neither read nor deleted. It is not a `netcheck` option yet.
- One run directory per scenario (`.work/run/netcheck-<scenario>`): two runs of the same scenario at the same time overwrite each other. Run the audit at gates, not during a wave.
- The sessions are short. A download that takes longer than the session is cut, and timers longer than 30 s (periodic checks) never fire: raise `--startup-wait`.
- What a language needs depends on the machine: a language server found on the `PATH` is used instead of downloaded, and Node is downloaded from `nodejs.org` only when the machine has none.

## Hosts measured on 2026-11-12

Measured again after the 2026-11-13 rename: `netcheck --scenario all --expect target` passes on the three scenarios, with the same hosts as in the tables below. Changed by the same work and not measured: upstream's usage reports to Anthropic, which were posted to `api.anthropic.com` whenever the user's own Anthropic key was used, are off (`crates/language_models/src/provider/anthropic/telemetry.rs` reports nothing). No scenario used an Anthropic key, so the tables below never showed that host.

**Update at gate 3 (2026-11-12, W3-10's runs on the 14:51 build; the tables below are wave 1's and are kept for comparison).** `netcheck --scenario startup --expect target` → PASS: `api.zed.dev` and `zed-extensions.nyc3.digitaloceanspaces.com` are gone from start-up (2 and 1 connections before); what remains is `localhost` ports 11434, 1234, 8080 (probes for local models, 5 connections, all unreachable) and `cdn.agentclientprotocol.com` (45 connections, the public agent registry; wave 4, W4-09, makes it lazy). `netcheck --scenario extensions --expect target` → PASS: `api.zed.dev` (2) and the storage host (1) appear only once the Extensions page is opened (the list, and the `html` extension the page then installs). The two registry hosts can also appear in any session that opens an `.html`, `.htm` or `.shtml` file (on-demand install of the listed `html` extension) and ten minutes after launch, at most once a day, while a registry extension is installed. `UNALIGNED_IDE_EXTENSIONS_URL` replaces the registry's base address. No scenario covers the on-demand install yet (the sample has no `.html` file).

Measured on 2026-11-12 on the macOS debug build of commit `8e3ba65ae2` plus the wave 1 working tree as it was at 09:19, with `script/unaligned/netcheck --scenario all --expect today` (verdict: PASS for the three). Reasons are from a reading of the code, completed by the app's log of each run.

### `startup`

| Host | Connections | Volume received | Why |
| --- | --- | --- | --- |
| `localhost` ports 11434, 1234, 8080 | 5, all unreachable | 0 | Probes for local models at startup: Ollama (`/api/tags`), LM Studio (`/api/v0/models`), llama.cpp (`/v1/models`), from `crates/language_models/src/provider/{ollama,lmstudio,llama_cpp}.rs`. Loopback only; nothing listens on this machine. |
| `cdn.agentclientprotocol.com` | 45 | 396 kB | The registry of external agents, fetched at every startup while the in-memory list is empty (`crates/project/src/agent_registry_store.rs`, started from `crates/ide/src/main.rs`): `registry.json`, then one icon per agent (44 today). Third party, not Zed. The plan makes it lazy (W4-09). |
| `api.zed.dev` | 2 | 10 kB | Zed's extension registry (`crates/extension_host/src/extension_host.rs`): the update check for installed extensions, sent at every startup, and the download request for the `html` extension, which `auto_install_extensions` installs on first launch. With that auto-install off, one connection remains. This is the baseline the plan removes from startup (W3-10). |
| `zed-extensions.nyc3.digitaloceanspaces.com` | 1 | 128 kB | The archive of the `html` extension: the download request on `api.zed.dev` redirects to this storage bucket. Not listed in research 01; operated for Zed's registry, so it is treated as a Zed host. |

### `extensions`

The same four hosts with the same connections. Opening the page sends its list request on a connection to `api.zed.dev` that is already open: no new connection, but the host is last active at 22 s and has received 386 kB instead of 10 kB. Under `--expect target` this scenario fails today, because the registry is contacted during the startup wait, before the page is opened.

### `chat`

The four startup hosts, plus:

| Host | Connections | Volume received | Why |
| --- | --- | --- | --- |
| `127.0.0.1` (port of the stand-in API) | 5 | 12 kB | The Unaligned provider: `GET /v1/models` and `GET /v1/balance` at startup because a key is present, then three `POST /openai/v1/chat/completions` (the message, the follow-up after the tool call, the title of the conversation). With a real key these go to `api.unaligned.pw`, the visible reason being the user's own key and message. 114 kB were sent: the system prompt, the tools and the file. |
| `api.github.com` | 1 | 1.1 MB | The scene opens `src/store.rs`; a Rust buffer starts rust-analyzer; none is installed in the instance, so the latest release is looked up (`crates/github_releases`, `crates/languages/src/rust.rs`). |
| `github.com`, `release-assets.githubusercontent.com` | 1 each | 14.4 MB | The download of that rust-analyzer release: `github.com` redirects to GitHub's storage for release files. |

A chat alone triggers the same download. In a run of the same steps with no file open (an earlier build of the same day), the three GitHub hosts were first seen at 23.7 s, just after the message: the agent's `read_file` opens `src/store.rs` as a buffer, and that starts the language server.

### With files open (not a scenario of its own)

`netcheck --scenario startup --expect today -- --open src/store.rs --open web/app.ts --open scripts/report.py --open config/settings.json --open README.md`, 22 s, on this machine (Node on the `PATH`, no usable rust-analyzer):

| Host | Connections | Volume received | Why |
| --- | --- | --- | --- |
| `api.github.com` | 3 | 3.6 MB | Release lookups for rust-analyzer, ruff, `package-version-server`, vscode-eslint. |
| `github.com` | 1 | 26 kB | Redirects to the downloads. |
| `release-assets.githubusercontent.com` | 2 | 25.9 MB | The binaries of rust-analyzer, ruff and `package-version-server` (a repository of Zed Industries on GitHub: a public download, no Zed server). |
| `codeload.github.com` | 1 | 492 kB | The source archive of `microsoft/vscode-eslint`. |
| `registry.npmjs.org` | 58 | 68.9 MB | npm installs: Prettier, vtsls, the JSON, Tailwind CSS and ESLint servers, basedpyright. |
| `nodejs.org` | 2 | 10.3 MB | Not the Node runtime (the log says "using Node.js found on PATH"): most likely an npm install step fetching Node headers. Not confirmed; on a machine without Node the runtime itself is downloaded from here. |

These hosts are the visible price of language support and stay in the target list.

## The proxy on its own

```sh
script/unaligned/netlog-proxy.py 0 .work/my.log     # prints the port it picked, serves until stopped
```

Python 3 standard library, 127.0.0.1 only. One line per connection asked for, and remarks starting with `#`:

```
2026-11-12T05:01:31.642Z GET localhost:11434 /api/tags
# 2026-11-12T05:01:31.650Z FAILED localhost:11434 Connection refused
2026-11-12T05:01:31.918Z CONNECT api.zed.dev:443
# 2026-11-12T05:02:03.324Z CLOSED api.zed.dev:443 sent=2025 received=381409 last=2026-11-12T05:01:53.013Z
```

Times are UTC. `CONNECT` tunnels carry the bytes untouched. Plain HTTP requests are forwarded with their headers and body; the log keeps the method, the host and the path, never the query string, a header or a body. To put any other program behind it: `HTTPS_PROXY=http://127.0.0.1:<port> HTTP_PROXY=http://127.0.0.1:<port> <command>`.
