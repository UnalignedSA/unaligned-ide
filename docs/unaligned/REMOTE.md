# Remote projects

How the application works on another machine: an SSH host, a WSL distribution, a dev container. Code: `crates/remote` and `crates/remote_connection` (the connection, upstream's), `crates/remote_server` (the program that runs on the host, upstream's), `crates/auto_update/src/remote_server.rs` (where that program comes from and how it is checked, this product's), `script/unaligned/bundle-remote-server` and the `remote-server` job of `.github/workflows/build.yml` (how it is built).

## What the user gets

File → Open Remote… (or `ssh://user@host/path` on the command line) opens a folder of the host as a project. Files are read and written on the host, the terminal is a shell of the host, search, git, language servers and the tasks run there, and so does what the AI does: the files it edits are the host's and the commands it runs are run on the host, with the rights of the SSH user.

It is not SFTP. The application puts one program on the host, the **remote server**, and talks to it through the SSH connection. Nothing has to be installed by hand and no administrator right is needed: the program goes into `~/.unaligned-ide_server/` of the SSH user, under a name that carries its version, and runs as that user. It listens on no network port: the application reaches it through SSH only.

Which hosts: Linux on x86_64 and on aarch64 (ARM). The server is one static executable (musl), so the host's distribution and C library do not matter. A macOS or Windows host has no server build: the connection says "release v… has no remote server for …".

## The Servers panel

The activity bar has a **Servers** icon. Its panel lists the SSH servers of the settings (`ssh_connections`); the code is `crates/recent_projects/src/servers_panel.rs`, `server_form.rs` and `server_store.rs`, and `crates/ssh_keys` for the keys.

- **A click on a server** connects and shows the folders of the host. The folder that is opened is remembered under the server; a click on it goes straight back. The server's icon is green while the window is on it.
- **The plus button**, or the pencil of a row, opens a form: a name, the address, the port, the user, and how to sign in. The address field takes what people are given: `203.0.113.4`, `root@host`, `root@host:2222`, or a whole `ssh root@host -p 2222`.
- **Sign in with a password**: typed in the form, it is kept in the system keychain and given to `ssh` when it asks. Left empty, it is asked at each connection. It is never written to the settings file.
- **Sign in with a private key**: one of the keys found in `~/.ssh`, a file chosen anywhere, text pasted from the clipboard, or a key made on the spot ("Create a New Key": an ED25519 key, protected when a passphrase is typed; the form shows its public line to add to the server's `~/.ssh/authorized_keys`, with a Copy button. The key is written when the server is saved). Every form people have keys in is read: OpenSSH, PEM (`RSA`, `EC`, PKCS#8), and PuTTY `.ppk` (versions 2 and 3), which is converted to an OpenSSH key; a PuTTY key protected by a passphrase gives an OpenSSH key protected by the same one. Line ends, a byte order mark, text around the key and a body that lost its line breaks are repaired. A public key chosen by mistake is said to be one.
- A key of `~/.ssh` that `ssh` can use as it is stays where it is. Any other key (pasted, converted, readable by others, from another folder) becomes a file of the application's own in `<config>/ssh_keys/`, readable by the user only, removed when no server uses it any more. The settings name the key with `-i <file> -o IdentitiesOnly=yes` in the entry's `args`, so `ssh` offers that key and not every key of the agent.
- **The first connection to a server** shows its fingerprint and a "Trust and Connect" button, in place of `ssh`'s question to answer with the word "yes". A saved password is never sent as the answer to that question, and is given once: if the server refuses it, the person is asked.

- **More Options** has two fields. *Jump host*: a server to go through first (`user@bastion:port`, several separated by commas), written as `-J` in the entry's `args`. *Port forwards*: ports of the server to open on this computer while connected, as `8080` (the same port), `3000:80`, or `5433:db.internal:5432`, kept in the entry's `port_forwards`.
- **The hosts of `~/.ssh/config`** are listed under the servers ("From your SSH config"), unless the setting `read_ssh_config` is off. A click adds the host to the servers and connects; `ssh` takes the address, the user, the key and the rest from the configuration file, as in a terminal.

Not there yet: putting a new key on the server for the person (the public line has to be added by hand), a password or a key of its own for the jump host (it uses what `ssh` finds by itself and asks in the connection window).

## Where the server comes from, and what is checked

The server has to be of the application's exact version: the two speak a protocol that changes between versions. So each release carries its own servers, beside the installers:

- `unaligned-ide-remote-server-linux-x86_64.gz`
- `unaligned-ide-remote-server-linux-aarch64.gz`

and lists them in its signed `update.json` (see `UPDATES.md`). When a host does not have the server of the running version yet, the application:

1. downloads `update.json` and `update.json.sig` of **its own version's** release (`…/releases/download/v<version>/`, an address built from the repository's name and the version, never taken from an answer) and checks the signature against the public keys compiled into it. The manifest must be this product's and that version's;
2. downloads the server to this machine and keeps it only when it has exactly the size and the SHA-256 the signed manifest gives. A download that grows past the signed size is stopped at once;
3. uploads that file to the host through the SSH connection, where it is unpacked (`gunzip`), made executable and moved to its place.

The checked file is kept on this machine (`remote_servers/` in the application's data folder), so a second host needs no second download. It is read and compared with the signed manifest again before every upload; the servers of earlier versions are removed.

Consequences:

- **A host is never given an address to download from.** Upstream lets the host fetch the server itself with `curl`; here the host could not check what it receives, so the application always downloads, checks and uploads. A host therefore needs no access to the internet, and the `upload_binary_over_ssh` setting changes nothing.
- **Someone who can change the files of a release but does not hold the update key cannot get anything onto a host**: a replaced server, a replaced manifest, a manifest of another version or no signature at all end in an error, and nothing is kept or uploaded.
- **A build without a releases repository or without a public key uploads nothing**: it could check nothing. A fork that wants remote projects makes its own key (`UPDATES.md`, "The key") and publishes its own releases.
- Whoever holds the private key can sign a server that every installed copy of that version would upload to its users' hosts. It is the same key, with the same care, as for the application's updates.
- A development build does not download: it compiles the server from the working tree (upstream's `IDE_BUILD_REMOTE_SERVER`).

Not covered by any of this: the host itself. An account that can write into `~/.unaligned-ide_server/` on the host can replace the program there; that account is the user's own.

## What the server does on the host

- It runs as the SSH user and stays in that user's rights. It writes under `~/.unaligned-ide_server/` and the product's own data, configuration and log folders of that user.
- It makes the requests the same features make on a desktop: downloading a language server or Node when a language needs one, an extension the project uses. It sends no telemetry and uploads no crash report (the application has no address to send either to: `NETWORK.md`).
- **What the AI may do applies on the host.** A command the agent runs is run on the host. With "Allow All" on, it is not asked about. Think about it before opening a production machine.

## Building it

`script/unaligned/bundle-remote-server`, on a Linux machine of the wanted architecture (`script/linux` installs what it needs, `musl-tools` included):

- target `<arch>-unknown-linux-musl`, `--release` with `.cargo/bundle-config.toml`: the same optimisation as the application (`remote_server` and every dependency at `opt-level = 3`, thin LTO, one code generation unit), statically linked;
- the channel file is set to `stable` for the build and put back;
- it refuses to package an executable that is not static, that does not start, that does not say the version of `crates/ide/Cargo.toml`, or that still carries upstream's server names;
- it writes `target/release/unaligned-ide-remote-server-linux-<arch>.gz` (`gzip -n`: the same executable gives the same file).

On GitHub the `remote-server` job of `build.yml` runs it twice, on `ubuntu-24.04` (x86_64) and on `ubuntu-24.04-arm` (aarch64), for a version tag, for `all`, or for a `build-…remote-server…` tag. The `draft-release` job puts both files in `update.json` before it is signed, checks them against it, and attaches them to the draft. A release whose server jobs failed is still drafted, with a warning: its application cannot open remote projects, and since a published version is never rebuilt, the cure is the next version.

## What was verified, and what was not

The Servers panel, on the development Mac on 2026-11-14, in private instances against SSH hosts in local containers: a server added through the form with a PuTTY key (the saved file read back by the real `ssh-keygen`, which gave the key's public half), a server edited to use a key of a stand-in `~/.ssh` and connected, a folder opened and found again under the server, a password kept and given after the first-connection screen, the settings file holding no password; a key created in the form with a passphrase (refused by the real `ssh-keygen` without it, read with it, and used to sign in to the test host once its public line was there), a jump host and port forwards written to the entry, a port forward answering on the Mac while connected, the hosts of a stand-in `~/.ssh/config` listed. `crates/ssh_keys` was checked against the real tools: 33 PuTTY files made by puttygen converted and read by OpenSSH 9.2 (29 by OpenSSH 10.5, which refuses DSA keys). Not tried: a connection through a jump host and a click on a host of the SSH configuration (both make `ssh` read the real `~/.ssh` of the machine), the Copy button, the file chooser and the paste button (the same code path as a found key from there on), the system keychain itself (the trial used a development build's file), Windows and Linux as the client.

The remote server: `PACKAGING.md`, "Executed and not executed", "The remote server". In short: built and tried end to end on the development Mac against a local SSH host and a stand-in for GitHub (connection, editing, terminal, a refused altered file); never built on GitHub, never tried from a release build, from Windows or Linux, or with WSL and containers.
