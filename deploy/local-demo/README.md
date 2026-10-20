# Linux demo preview

This folder supplies the local demo launch/check scripts. The assembled archive also contains
the real server, CLI and converter binaries, built browser/MCP Apps assets, verified conversion
models, a file-hash manifest and the observed demo-readiness report. It contains no installation
data, browser session, connector credential or AI account key. It is not an installer or a
completed release-qualification claim.

Download the archive and checksum from the
[Linux demo prerelease](https://github.com/freebatteryfactory/okf-jawn/releases/tag/linux-demo-2026-10-20).
The bundle includes its verified model inventory and third-party notices. It needs x86-64
Linux with glibc 2.39 or newer (Ubuntu/Pop!_OS 24.04-era systems); no Rust, Bun or Docker build
is needed on the laptop.

From an empty download directory:

```sh
curl -fLO https://github.com/freebatteryfactory/okf-jawn/releases/download/linux-demo-2026-10-20/okf-jawn-linux-x86_64-glibc239-20261020.tar.gz
curl -fLO https://github.com/freebatteryfactory/okf-jawn/releases/download/linux-demo-2026-10-20/okf-jawn-linux-x86_64-glibc239-20261020.tar.gz.sha256
sha256sum --check okf-jawn-linux-x86_64-glibc239-20261020.tar.gz.sha256
tar -xzf okf-jawn-linux-x86_64-glibc239-20261020.tar.gz
cd okf-jawn-linux-x86_64-71b9127a
./check.sh && ./start.sh
```

The rehearsal of application revision `71b9127a` built all three binaries on x86-64 Ubuntu
24.04.4 under WSL. Seventeen model files matched the recorded hashes. Real DOCX-with-picture,
PDF and XLSX imports and pinned reads passed; Windows Chromium rendered the Linux server's
PDF canvas and spreadsheet original. The complete protected draft/review/proposal journey,
Linux stdio MCP discovery and pinned read, and an orderly restart with retained content also
passed. External-driver failures and their scoped repairs remain recorded in the bundle's
readiness report. No application repair was required. This is not a Pop!_OS-native or real
AI-model/client pass.

## On the laptop

1. Inspect `cat /etc/os-release` and `uname -m`. Extract the archive into a directory you own.
2. From that directory run `./check.sh`. This verifies the transferred files and runtime
   libraries without opening the application store. The generated `runtime-requirements.txt`
   states the minimum glibc version measured from this build. An Ubuntu 24.04 build may not run
   on Pop!_OS 22.04. Do not upgrade system glibc by hand; use a compatible build instead.
3. Run `./start.sh` in a terminal. Open the single-use local sign-in link it prints. The app
   listens only on `127.0.0.1:7711`; its separate sandbox listens on port 7712. No WorkOS, public
   URL or cloud deployment is used.
4. Create a workspace and import the files under `samples/`. Try the text/outline/pictures/
   pages/original choices that the document supplies, PDF paging, spreadsheet ranges and a
   correction. Save a note, make a Snapshot, reload and inspect its history.
5. Stop the server with Ctrl-C and start it again; verify the same workspace reopens.

The store defaults to `${XDG_DATA_HOME:-$HOME/.local/share}/okf-jawn-demo`, separate from other
installations. To choose another store, set the absolute `OKF_DEMO_DATA_DIR` before starting.
Keep that directory private. To obtain a fresh sign-in link after the printed link expires,
read its `launch-url` file locally; never paste it into a shared report. An occupied port or
locked data directory is a refusal, not a reason to start a second writer or disable checks.

## Local MCP agent

In the workspace's administration screen issue a connector scoped to that workspace with
Read and, for the proposal demo, Propose. Save the once-shown secret in a private file outside
this bundle (for example `~/.config/okf-jawn-demo/connector`, mode `0600`). Do not put its value
in command arguments, a screenshot, source control or an AI prompt.

Configure the agent to run the **absolute path** to `start.sh`, with arguments:

```text
--mcp --token-file /absolute/path/to/connector
```

The wrapper uses the actual `okf-jawn mcp --stdio` relay, not filesystem access or an alternate
store. Start the browser server first for the initial setup. The relay can subsequently start
the same local service if it is absent, using the same environment/data directory.

For Codex CLI the registration shape is:

```sh
codex mcp add okf-demo -- /absolute/path/to/start.sh --mcp --token-file /absolute/path/to/connector
```

Ask the agent to list the workspace, find a phrase in an imported document, read and cite its
source, then propose a small note change. Inspect and accept that exact proposal in the UI.
The connector cannot review or accept it. Revoke the connector after the demo. A real AI-client
pass is complete only after these calls have actually succeeded; protocol tests are not that
claim. Browser ChatGPT/tunnel setup is a separate, account-authorized step.

## Limits

Read `demo-readiness.json` in the assembled bundle for what actually ran, its source revision,
platform and failures. WSL Ubuntu evidence does not certify Pop!_OS or its native browser.
Single-item purge and the View-export job remain unfinished. The known SVG document-isolation
concern is tracked in issue #114; the HTML sandbox pass does not qualify that SVG path.

The included models are the owner's hash-verified copy of the original qualified bytes,
with their upstream sources and notices recorded in `models-manifest.json` and `notices/`.
An upstream mutable model tag is not used to discover replacements. This preview does not
establish the full installer/image release campaign. No dependency downloads run at startup.
