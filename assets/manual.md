# Unaligned IDE: user guide

A short guide to the editor: how to start, how to connect a model, how to use the chat, how to work on a server, and what to do when something goes wrong. It ships inside the application (Help → User Guide), so it describes the version you are running.

Shortcuts are written for Windows and Linux. On macOS, use `Cmd` where this guide says `Ctrl`, unless a line says otherwise.

- [First start](#first-start)
- [The window](#the-window)
- [Connect a model](#connect-a-model)
- [The chat](#the-chat)
- [Working in a project](#working-in-a-project)
- [Servers (SSH)](#servers-ssh)
- [Settings, themes and keys](#settings-themes-and-keys)
- [Updates](#updates)
- [Privacy](#privacy)
- [Shortcuts](#shortcuts)
- [When something goes wrong](#when-something-goes-wrong)

## First start

The builds are not signed for the operating system yet, so each system asks once before the first start.

- **Windows**: run the installer. SmartScreen shows a blue screen: choose "More info", then "Run anyway". No administrator rights are needed.
- **macOS**: open the disk image and drag the application to Applications. macOS then says it "could not verify" the application. Either open System Settings → Privacy & Security and choose "Open Anyway", or run this once in the Terminal:

  ```sh
  xattr -dr com.apple.quarantine "/Applications/Unaligned IDE.app"
  ```

- **Linux**: make the AppImage executable (`chmod +x unaligned-ide-linux-x86_64.AppImage`), then start it.

The first window is a welcome page: pick a theme, open a folder, connect a model. Help → Show Welcome brings it back.

## The window

The layout is the one of VS Code.

- **Activity bar** (the column of icons on the left): one icon per panel. Explorer, source control, servers, outline, chat, terminal, debugger. A click shows or hides the panel. At the bottom: the AI connections and the Manage menu (settings, keyboard shortcuts, themes, extensions).
- **Panels** can be moved: drag a panel's icon to the left, the right or the bottom of the window.
- **Layout buttons** at the right end of the title bar show or hide the left, bottom and right sides, and offer three ready-made layouts: Classic, AI-First and Zen.
- **Command centre** (the search box in the middle of the title bar): click it to find a file by name.
- **Command palette**: `Ctrl+Shift+P`. Every command of the editor is there, by name. When you do not know where something is, type what you want.
- **Status bar** (bottom): the git branch and problems on the left; on the right the zoom buttons (`−`, the percentage, `+`), the cursor position, the language of the file, and the AI connection with what is left on your Unaligned key.
- **Menus**: on Windows and Linux they are at the left of the title bar, after the product's mark: File, Edit, Selection, View, Go, Run, and "…" for the others. On macOS they are in the system's menu bar.

**Zoom** changes the size of the code: `Ctrl` `+`, `Ctrl` `-`, `Ctrl` `0` to go back to 100%, or the buttons of the status bar. The zoom is kept after a restart.

## Connect a model

Open the chat with `Ctrl+L`. Until something is connected, it shows the connection screen. You can connect several things and choose for each chat.

- **Unaligned**: create a key on [unaligned.pw](https://unaligned.pw/dashboard/keys) and paste it. The key is checked, your balance shows, and the list of models loads with their prices.
- **A subscription you already pay for**: ChatGPT, GitHub Copilot, SuperGrok, or a vendor's own agent (Claude Code, Codex, Gemini CLI) inside the same chat.
- **Your own key**: OpenAI, Anthropic, Google, Mistral, DeepSeek, xAI, OpenRouter, Amazon Bedrock, or any address that speaks the OpenAI format.
- **A local model**: Ollama, LM Studio or llama.cpp running on your machine. Free, and nothing leaves the machine.

To add or change a connection later: the chat's "…" menu, "Connections…", or the AI text at the right end of the status bar.

Keys are kept in the system's keychain (Keychain on macOS, Credential Manager on Windows, the secret service on Linux), never in a settings file.

## The chat

One chat, two modes, chosen in the chat:

- **Agent** reads and edits your files and runs commands to do what you ask.
- **Ask** only answers. It changes nothing.

Three shortcuts:

| Keys | What it does |
| --- | --- |
| `Ctrl+L` | Opens the chat, with the selected code attached |
| `Ctrl+K` | Edits the code under the cursor from a sentence you type |
| `Ctrl+I` | Starts a new chat |

**You stay in control.** Every edit the agent makes is shown for review: keep it or undo it, for one file or for all. Before it uses a tool (running a command, editing a file), the agent asks. "Allow All" stops those questions when you trust the task; a few dangerous commands are refused whatever the setting.

**Cost.** With Unaligned, each chat shows what it has cost and the status bar shows what is left on the key.

**When a message fails**, the chat says why in plain words (not connected, key refused, balance too low, too many requests, server down) and what to do.

## Working in a project

- **Open a folder**: File → Open…, or drag a folder onto the window.
- **Find a file**: `Ctrl+P`. **Search in all files**: `Ctrl+Shift+F`.
- **Explorer**: `Ctrl+Shift+E`. Right-click a file or a folder for new, rename, delete and the rest.
- **Terminal**: ``Ctrl+` `` opens it at the bottom, in the project's folder.
- **Source control**: the branch icon of the activity bar. Stage, commit, push, pull and see what changed.
- **Go to definition** `F12`, **rename a symbol** `F2`. Code intelligence comes from language servers, which the editor downloads the first time a language needs one.
- **Extensions**: the Manage menu → Extensions adds languages and themes.
- **Debugger**: the bug icon of the activity bar.

A project can carry its own settings, tasks and debug scenarios in a `.unaligned/` folder at its root.

## Servers (SSH)

The Servers icon of the activity bar lists the machines you work on over SSH. On a server, the files, the terminal, the search, source control and the AI all work on that machine.

**Add a server**: the `+` button. Fill in a name, the address, the port and the user. The address field takes what you were given as it is: `203.0.113.4`, `root@host`, `root@host:2222`, or a whole `ssh root@host -p 2222`.

**Sign in** with a password or with a private key:

- **Password**: typed here, it is kept in the system's keychain. Left empty, it is asked at each connection.
- **Private key**: pick one of the keys found on this computer (`~/.ssh`), choose a file, or paste the key. OpenSSH, PEM and PuTTY (`.ppk`) keys are all read; a PuTTY key is converted when you save. "Create a New Key" makes a new one: add the line the form shows to the file `~/.ssh/authorized_keys` of your user on the server before you connect.

"More Options" has a **jump host** (a server to go through first) and **port forwards** (a port of the server opened on this computer: `8080`, or `3000:80`).

**Connect**: click the server. The first time, the editor shows the server's fingerprint: "Trust and Connect" if you expect this server. The window then moves to the server and the panel shows its files, from your home folder and from `/`.

- Click a **folder** to open or close it, a **file** to open it in the editor.
- The button on a folder's row, **Work in this folder**, makes it your project: the explorer, the search, source control and the AI then work inside it. The folder is remembered under the server for next time.

The first connection to a server puts a small program in `~/.unaligned-ide_server/` of your user on that server. It needs no administrator rights and opens no network port. The server must run Linux (x86_64 or ARM).

**Mind the AI on a server**: a command the agent runs is run on the server. With "Allow All" it does not ask first.

## Settings, themes and keys

- **Settings**: `Ctrl+,`. Everything is also in a text file: type "open settings file" in the command palette.
- **Themes**: `Ctrl+K` then `Ctrl+T`. Five built-in themes (Unaligned, Light, Dusk, Midnight, Contrast), plus One, Ayu and Gruvbox. The accent colour and the corners are settings.
- **Keyboard shortcuts**: `Ctrl+K` then `Ctrl+S` lists every shortcut and lets you change them. The defaults are VS Code's.

## Updates

Once a day the application asks GitHub whether a newer version exists. When there is one, the title bar shows **Update Available**. A click downloads it, checks that it is signed by this project, and installs it; **Restart to Update** then starts it. Nothing is downloaded before your click.

- To check now: Check for Updates… in the product's menu (the mark at the left of the title bar on Windows and Linux, the application's name in the menu bar on macOS).
- To turn the daily check off: `"auto_update": false` in the settings.

## Privacy

No account, no telemetry, no crash reports. The application contacts:

- the model you connected, with what you send it;
- GitHub, once a day, for the update check (an anonymous request);
- the places a feature you use needs: an extension you install, a language server a language needs.

Nothing about your code or your usage is sent anywhere else.

## Shortcuts

On macOS, `Cmd` replaces `Ctrl` except where the last column says otherwise.

| Action | Windows and Linux | macOS |
| --- | --- | --- |
| Command palette | `Ctrl+Shift+P` | `Cmd+Shift+P` |
| Find a file | `Ctrl+P` | `Cmd+P` |
| Search in all files | `Ctrl+Shift+F` | `Cmd+Shift+F` |
| Open the chat | `Ctrl+L` | `Cmd+L` |
| Edit code from a sentence | `Ctrl+K` | `Cmd+K` |
| New chat | `Ctrl+I` | `Cmd+I` |
| Explorer | `Ctrl+Shift+E` | `Cmd+Shift+E` |
| Show or hide the left side | `Ctrl+B` | `Cmd+B` |
| Terminal | ``Ctrl+` `` | ``Ctrl+` `` |
| Save | `Ctrl+S` | `Cmd+S` |
| Settings | `Ctrl+,` | `Cmd+,` |
| Themes | `Ctrl+K` `Ctrl+T` | `Cmd+K` `Cmd+T` |
| Keyboard shortcuts | `Ctrl+K` `Ctrl+S` | `Cmd+K` `Cmd+S` |
| Zoom in, out, reset | `Ctrl` `+`, `-`, `0` | `Cmd` `+`, `-`, `0` |
| Go to definition | `F12` | `F12` |
| Rename a symbol | `F2` | `F2` |

## When something goes wrong

**The chat does not answer.** Read the message in the chat: it names the cause. For an Unaligned key, check the balance in the status bar; a low balance is topped up on [unaligned.pw](https://unaligned.pw/dashboard/billing).

**A server does not connect.**

- "Permission denied": the user, the password or the key is not the one the server expects. With a new key, check that its public line is in `~/.ssh/authorized_keys` on the server.
- "has no remote server for …": the server is not a Linux machine on x86_64 or ARM.
- The address does not answer: check it from a terminal with `ssh user@address`.

**macOS asks for a password to read a saved key** after an update or a new installation: type the password of your Mac session and choose "Always Allow". It is the Mac's keychain checking that the application may read what it saved.

**Something else.** The log says what the application was doing: open the command palette and type "open log".

### Report a bug

Help → Report a Bug… opens a form on the project's GitHub page, with your version and your system already filled in. Nothing is sent before you press the button there, and you see everything that will be sent. Say what you did, what you expected and what happened; a screenshot helps.

- The reports are public: do not paste keys, passwords, server addresses or private code. Read the log before you attach any of it.
- No GitHub account, or something private to say: Help → Contact Support opens the contact page of [unaligned.pw](https://unaligned.pw/contact).
- An idea: Help → Request a Feature….

The application sends no report by itself, so a problem nobody writes about is a problem nobody knows.
