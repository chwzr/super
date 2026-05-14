# A Comprehensive Guide: Driving and Inspecting TUI Apps from Claude Code (2025–2026)

Claude Code's built‑in `Bash` tool does **not** allocate a real pseudo‑terminal — it runs commands in a non‑interactive pipe‑backed shell that returns stdout/stderr after the process exits (or, with `run_in_background: true`, lets you poll output via `BashOutput`). That means any program that requires a real TTY (curses/ncurses apps, full‑screen TUIs, REPLs that probe `isatty`, full‑screen editors like vim, debuggers like `lldb`/`pdb` in their fancier modes, `htop`, `lazygit`, etc.) will either refuse to run, error out (`Inappropriate ioctl for device`, "raw mode is not supported", "stdin is not a tty"), or render garbage.

The community has converged on a small handful of patterns that work around this. The dominant one — by a very wide margin — is **tmux running detached, driven via `tmux send-keys` and `tmux capture-pane`**. Below is a survey of every viable approach in 2025–2026, with concrete commands and source references, followed by a recommended recipe.

---

## 1. The tmux pattern (the de facto standard)

The technique is simple: start a tmux server in detached mode, launch the TUI inside it, then drive it from Claude Code's normal `Bash` tool by shelling out `tmux send-keys` and `tmux capture-pane`. Tmux is the PTY layer — your TUI thinks it is running on a real terminal of a known size, and Claude Code only ever sees plain text snapshots of the screen.

This pattern is endorsed by Anthropic (it is the recommended workflow in their own Claude Code best‑practices doc), is the basis of Jesse Vincent's popular `superpowers-lab` skill `using-tmux-for-interactive-commands`, and is what the Hermes Agent, NousResearch's `hermes-agent`, and many other agent frameworks use to orchestrate Claude Code itself.

### Canonical command patterns

```bash
# 1. Start the TUI in a detached tmux session with a known size
tmux new-session -d -s tui -x 200 -y 50 'my-tui-app'

# 2. Let it initialize (almost always required — see Gotchas)
sleep 0.5

# 3. Take a "screenshot" of the current visible pane (plain text)
tmux capture-pane -t tui -p

# 3b. Take a screenshot including ANSI color/attribute escape sequences
tmux capture-pane -t tui -e -p

# 3c. Include scrollback (last 200 lines of history above the visible screen)
tmux capture-pane -t tui -p -S -200

# 3d. Join wrapped lines and trim trailing blanks (tmux ≥ 3.4 has -T)
tmux capture-pane -t tui -pJ -S -

# 4. Send a literal string
tmux send-keys -t tui 'hello world'

# 5. Send special / control keys (these are tmux's symbolic names)
tmux send-keys -t tui Enter
tmux send-keys -t tui Up Up Down Enter
tmux send-keys -t tui Escape ':wq' Enter
tmux send-keys -t tui C-c            # Ctrl-C
tmux send-keys -t tui C-x C-s        # Ctrl-X Ctrl-S
tmux send-keys -t tui M-x            # Alt-X
tmux send-keys -t tui Space          # space
tmux send-keys -t tui BSpace         # backspace
tmux send-keys -t tui F1 F12         # function keys
tmux send-keys -t tui PageUp PageDown Home End

# 6. Force a redraw if you suspect the captured screen is stale
tmux send-keys -t tui C-l

# 7. Clean up
tmux kill-session -t tui
```

### How "observe → act → observe" loops actually work

The agent loop is just a shell loop:

```bash
# Observe
SCREEN="$(tmux capture-pane -t tui -p)"
# (Claude reads SCREEN, decides what to do)

# Act
tmux send-keys -t tui Down Down Enter

# Wait for stable output, then observe again
sleep 0.3
tmux capture-pane -t tui -p
```

A more robust "wait for the screen to stop changing" loop, which several skills (including `obra/superpowers-lab` and `tui-use`) recommend in place of fixed sleeps:

```bash
prev=""
for i in 1 2 3 4 5 6 7 8 9 10; do
    cur="$(tmux capture-pane -t tui -p)"
    if [ "$cur" = "$prev" ] && [ -n "$cur" ]; then
        break    # screen has been stable for one tick
    fi
    prev="$cur"
    sleep 0.15
done
echo "$cur"
```

### The `-e` (ANSI) flag — when colors matter

`tmux capture-pane -p` strips all SGR/escape codes by default and returns plain text. For most agent loops that is exactly what you want, because color codes are noise the LLM does not need. But if the TUI conveys meaning *through color alone* (status indicators, syntax highlighting, terminal games, Dwarf Fortress‑style maps where color = material), pass `-e` to keep the escapes:

```bash
tmux capture-pane -t tui -e -p          # includes \e[31m, \e[1m, etc.
tmux capture-pane -t tui -eJ -p -S -    # plus scrollback, joined lines
```

`brendanlong/claude-code-plays-text-games` switched to `-e` precisely because Claude needed to distinguish materials by color in Dwarf Fortress. If you want HTML for visual review later, pipe the `-e` output through `ansi2html`.

### Useful `capture-pane` flags (tmux ≥ 3.3)

| Flag | Effect |
|---|---|
| `-p` | print captured text to stdout (without this, it goes to a paste buffer) |
| `-e` | include ANSI escape sequences (colors, bold, etc.) |
| `-J` | join wrapped lines into single lines |
| `-S -N` / `-S -` | start N lines (or all) above the visible screen — includes scrollback |
| `-E N` | end at line N (negative = lines above bottom) |
| `-a` | alternate screen content (useful for vim/less which use altscreen) |
| `-N` | preserve trailing whitespace |
| `-C` | escape non‑printables as octal `\xxx` |
| `-T` | trim trailing empty cells (tmux 3.4+) |

### Sending bulky / non‑ASCII text via paste buffer

`send-keys` is fine for short input. For pasting larger or multi‑line text, set a paste buffer and paste it — this avoids quoting nightmares and works around line‑editing weirdness:

```bash
tmux set-buffer -b mybuf -- "$(cat large_input.txt)"
tmux paste-buffer -t tui -b mybuf
tmux delete-buffer -b mybuf
```

This trick is documented in David R. MacIver's "Using tmux to test your console applications" (2015), which is essentially the founding article of the agent‑driving‑tmux pattern.

### Isolating the tmux server (critical for agents on shared machines)

If you run Claude Code inside its *own* tmux session and then spawn another tmux session for the TUI, you can collide with the user's tmux config and key bindings. The well‑tested fix used by serious skills (e.g. MCP Market's `cli-tmux` skill) is to use a **dedicated tmux server socket** with `-L`:

```bash
SOCK=claude-tui
tmux -L "$SOCK" -f /dev/null new-session -d -s tui -x 200 -y 50 'my-tui-app'
tmux -L "$SOCK" send-keys -t tui Down Enter
tmux -L "$SOCK" capture-pane -t tui -p
tmux -L "$SOCK" kill-server   # nukes only this isolated server
```

`-f /dev/null` ignores the user's `~/.tmux.conf` (so the user's prefix bindings, plugins, statusline etc. don't interfere) and `-L claude-tui` puts the server on its own Unix socket so it cannot accidentally interact with sessions the human has open.

### Known gotchas

These bite essentially everyone the first time:

1. **Always sleep after `new-session` before the first `capture-pane`.** TUIs take 100–500 ms to draw their initial frame; capturing too soon returns a blank screen. The `using-tmux-for-interactive-commands` skill recommends `sleep 0.3` minimum.
2. **`Enter` is a separate argument, not `\n`.** `tmux send-keys 'foo\n'` types the literal four characters `f`, `o`, `o`, `\n`. Correct: `tmux send-keys 'foo' Enter`.
3. **Quoting.** `send-keys 'a b c'` sends `a b c`; `send-keys a b c` sends three separate keys `a`, `b`, `c`. For literal arguments use `-l`: `tmux send-keys -l 'something with $special chars'`.
4. **Fixed pane size.** Always pass `-x` and `-y` to `new-session`. Without an attached client, tmux defaults to 80×24, which truncates many modern TUIs and breaks layout assertions. Common choices are `-x 200 -y 50` or `-x 120 -y 40`.
5. **`run_in_background` does not give you a TTY.** Even in background mode Claude Code's `Bash` tool does not allocate a PTY — see §4. tmux's `new-session -d` does, which is exactly why this pattern works.
6. **Alternate screen buffer.** Many TUIs (vim, less, htop) use the xterm alternate screen. `tmux capture-pane -p` captures the alt screen by default while the program is running, which is what you want. If you want to see what's underneath, use `-a`.
7. **Don't run nested tmux inside Claude Code without `TMUX=` clearing or `-L`.** Nested tmux is supported but the inner server needs distinct sockets to avoid prefix/key collisions.
8. **iTerm2's `tmux -CC` integration breaks alternate‑screen rendering** for Claude Code's own UI; use plain tmux instead (hboon.com).
9. **macOS DSR cursor‑report bug.** A known issue (anthropics/claude-code #17787, Jan 2026) caused Claude Code's *own* TUI to leak `^[[35;1R` cursor‑position reports on macOS — the workaround was to run Claude Code itself inside tmux. Unrelated to driving *your* TUI from Claude, but worth knowing if you see escape codes showing up in your input.

### Why tmux beats just about everything else for this use case

* Works identically on macOS and headless Ubuntu (no X server needed — see §7).
* Survives Claude Code restarts: a detached tmux session lives as long as `tmux kill-server` hasn't been called, so an agent can come back later and pick up where it left off.
* `capture-pane` returns a fully‑rendered terminal snapshot — escape codes have already been interpreted into a 2‑D grid of characters. This is *exactly* the right level of abstraction for an LLM, because the LLM sees what a human would see, not the raw VT100 byte stream.
* Almost every coding agent already knows the tmux CLI from its training data, so prompting cost is low.
* You can attach to the same session with `tmux -L claude-tui attach -t tui` from another terminal and watch the agent work in real time.

---

## 2. expect / pexpect approaches

Don Libes' `expect` (Tcl) and the Python port `pexpect` are the classical answer to "drive an interactive program". They allocate a PTY, write input, and pattern‑match output against regexes.

```python
import pexpect
child = pexpect.spawn("my-tui-app")
child.expect("Username: ")
child.sendline("alice")
child.expect(r"\$ ")
child.sendline("ls")
print(child.before)
```

### When pexpect/expect is the right tool

* The program is a *line‑oriented prompt*, not a full‑screen curses app — installers, SSH, FTP, password prompts, simple REPLs, `git rebase -i` editor prompts.
* You want to assert "the program asked for X, I sent Y" rather than "the screen looks like this".
* You're writing a deterministic, scripted automation (CI, deployment), not an interactive observe‑act loop.

### Why it's worse than tmux for TUIs

The Hypothesis author David R. MacIver wrote a now‑canonical post explaining why he gave up on pexpect for testing TUIs and switched to tmux: pexpect's built‑in ANSI screen emulator (`pexpect.screen`/`pexpect.ANSI`) is partial and buggy for anything with cursor movement, alternate screens, or complex redraws. The pexpect docs themselves deprecate those modules and steer you toward `pyte` if you must render a VT100 stream yourself.

In practice:

* For **menus, arrow‑key navigation, full‑screen redraws** → tmux is dramatically more reliable because tmux *is* a real terminal emulator.
* For **dump prompt/response exchanges** (`Are you sure? [y/N]`) → pexpect is more concise and gives you regex matching for free.
* If you want both, `jnurmine/tmux-echelon` is a small project that drives a tmux pane *through* a pexpect script via `pipe-pane`, combining tmux's accurate rendering with pexpect's regex flow control.

Claude Code is perfectly capable of writing a pexpect script and running it via Bash, so this is a viable secondary option — but for any program with screen‑oriented redraws, prefer tmux.

---

## 3. MCP servers that expose a real PTY

A small but growing ecosystem of MCP (Model Context Protocol) servers gives Claude (especially Claude Desktop, but also Claude Code via `claude mcp add`) direct, structured access to terminal sessions. These are alternatives to "shell out to tmux from the Bash tool" — they expose `start_session`, `send_keys`, `capture_pane`, etc. as MCP tools.

### Notable servers (all available on GitHub, 2024–2026)

| Server | Repo | Backend | Notes |
|---|---|---|---|
| **terminalcp** | `badlogic/terminalcp` (npm `@mariozechner/terminalcp`) | `node-pty` + `xterm.js` headless renderer | Single‑tool MCP design ("terminalcp" with action argument), persistent daemon, attach support; explicitly built for agents driving lldb/vim/htop. Author's blog post benchmarks it vs tmux/screen/CLI. |
| **tmux-mcp (nickgnd)** | `nickgnd/tmux-mcp` | wraps real `tmux` binary | Originally for Claude Desktop, exposes session list/capture/send. |
| **tmux-mcp (MediocreTriumph)** | `MediocreTriumph/tmux-mcp` | wraps `tmux` | Cross‑platform (macOS/Linux/WSL2), exposes `create_session`, `send_keys`, `capture_pane`, `split_window`, etc. |
| **mcp-tmux** | `kazuph/mcp-tmux` | wraps `tmux` | TypeScript, focused on Claude Desktop. |
| **terminal-mcp** | `elleryfamilia/terminal-mcp` | own PTY emulator | Has sandboxing + asciicast recording for replay. |
| **agent-tui** | `pproenca/agent-tui` (Rust) | own VT emulator | Adds Playwright‑style element refs (`@e1`), screenshots in text or JSON, websocket live preview. |
| **tmux-claude-mcp-server** | `michael-abdo/tmux-claude-mcp-server` | wraps `tmux` | Hierarchical orchestration of multiple Claude Code instances. |
| **tui-use** | `onesuper/tui-use` (npm) | own PTY daemon | Headless xterm renderer; smart `wait --text` / `wait --stable` instead of sleeps. "BrowserUse for the terminal." |

### Which one to pick

* If you want **the cleanest single‑install MCP** and you trust npm, **`terminalcp`** is the most mature. Install with `code --add-mcp '{"name":"terminalcp","command":"npx","args":["@mariozechner/terminalcp@latest","--mcp"]}'` (or the equivalent `claude mcp add` invocation). It uses `node-pty` to spawn the process and `xterm.js`'s headless renderer to maintain a real VT100 grid, then lets the LLM ask for either the rendered screen (`stdout`) or just new bytes since last poll (`stream` with `since_last`). Mario Zechner's own benchmarking found terminalcp and tmux were roughly tied for success rate on three agent tasks (start process / send input / read output); both hit 100% on Claude. His conclusion: "the protocol is just plumbing — what matters is whether your tool helps the agent". So you can pick whichever fits your stack.
* If you want **zero new dependencies** and your machine already has tmux, just use the Bash + tmux pattern from §1. That is what most production setups (Anthropic's own internal skills, Hermes Agent, NousResearch, obra/superpowers) actually do.
* If you want **MCP + Playwright‑style semantic element refs**, look at `agent-tui` — it parses the rendered screen into a tree and gives elements stable IDs, which can simplify multi‑step interactions.

### Why MCP vs. CLI mostly doesn't matter

Anthropic has cautioned (and the terminalcp blog post documents) that *adding many MCP tools dilutes the LLM's attention*. A single well‑documented `Bash` invocation of `tmux send-keys` performs as well as a dedicated MCP. The genuine advantage of MCP servers is **persistent server state between Claude Code restarts** and **typed/structured responses**. If neither matters for your use case, plain `tmux` over `Bash` is simpler.

---

## 4. Claude Code's own background‑bash features (and their limits)

Claude Code's `Bash` tool documentation (the current version on `docs.claude.com` and `platform.claude.com`) confirms these features:

* **`run_in_background: true`** — runs the command without blocking, returns immediately with a `bash_id`/shell ID. You then poll output with the `BashOutput` tool (which returns only **new** output since the last read) and kill with `KillBash`. In the interactive REPL you can also list them with `/bashes`.
* **Persistent shell session** — Claude Code keeps a single bash session between tool calls so env vars and `cd` persist. This is *not* a PTY; it's a child shell with pipes for stdio.
* **Timeout** — default 120 s, configurable up to 600 s (10 min).
* **30 KB output truncation** — long output is truncated before being returned to the model.
* **`MAX_MCP_OUTPUT_TOKENS`** env var caps MCP server output.

### The PTY question — confirmed: Claude Code's bash tool does NOT allocate a TTY

This is explicit in multiple GitHub issues:

* **#9881 ([FEATURE] Add Interactive Shell Support via PTY)** — community feature request, still open, asking Anthropic to add `node-pty` backing. The current state of `Bash` is described in the issue: "Currently, the Claude Code Bash tool is incredibly powerful for non‑interactive work, but cannot run interactive programs." The proposed fix is to wrap commands in a real PTY (node-pty) and stream xterm‑rendered frames.
* **#26353 ([FEATURE] Interactive TTY mode for bash)** — quotes Claude itself: *"the Bash tool doesn't have a TTY or interactive input."*
* **#8078 ([BUG] External tools not run in interactive TTY)** — Vitest defaults to watch mode when it sees a TTY and to single‑run mode when it doesn't, and Claude's bash invocation does not register as a TTY.

So `run_in_background` lets you keep a *non‑interactive* process alive and stream new bytes from it, which is great for `npm run dev`, `tail -f`, or test runners that print to stdout, but it **will not work** for any curses‑style TUI that needs `ioctl(TIOCSWINSZ)` or raw mode.

### The standard workaround inside Claude Code

The workaround Anthropic engineers use internally — and which the official "Skills for Claude Code" guide (a widely shared Medium post by an Anthropic engineer) explicitly cites as a category of skill — is `tmux-cli-driver: interactive CLI testing that requires a TTY`. Concretely:

* The agent launches `tmux new-session -d ...` via the regular Bash tool. tmux *itself* allocates the PTY and runs the TUI inside it, so the lack of a TTY on Claude Code's own bash is irrelevant.
* The agent then uses ordinary, non‑interactive `tmux send-keys` and `tmux capture-pane -p` invocations through the same Bash tool. These do not need a TTY because they communicate with the tmux server via its Unix socket.

This is the "developer work‑around" the question asks about, and it is the single most common one.

---

## 5. Screenshot‑based / visual approaches

For cases where text capture isn't enough — apps that use complex Unicode box‑drawing where alignment matters, apps that depend on 24‑bit color, or apps you want to *demo* to a human — there are several routes.

### asciinema (.cast files)

`asciinema rec session.cast -c 'my-tui-app'` records a timestamped stream of terminal output in the asciicast v2 format (a JSON‑lines text format). Because the file is structured text, agents can read it — Tim Rambo demonstrated this with Hermes Agent. Combine with `asciinema play` for human review and `asciinema cat` to dump the full text.

A more interesting move: `elleryfamilia/terminal-mcp` writes asciicast files automatically as it drives sessions, so you get an auditable replay for free.

### Charm VHS (`charmbracelet/vhs`)

`vhs` reads a `.tape` script (`Type "..."`, `Enter`, `Sleep 500ms`, etc.) and produces a GIF — *and* it can emit a plain `.txt` or `.ascii` file of the final screen for integration testing. VHS is specifically designed to be CI‑friendly and "GIFs as code", and a community thread on `charmbracelet/vhs#715` is actively asking the maintainers to expose VHS as an AI skill. Today you can already have Claude Code write a `.tape` and run `vhs`, then `Read` the resulting `.txt` to "see" the rendered screen.

### teatest (for Go Bubble Tea apps specifically)

`github.com/charmbracelet/x/exp/teatest` is the official testing harness for Bubble Tea TUIs. It spins up the tea.Program in a headless virtual terminal and lets you assert against the rendered output via golden files. If your TUI is a Bubble Tea app, this is more reliable than any black‑box driver because it runs the *real* event loop with no PTY required.

### Headless real terminals — Xvfb + xterm/kitty/alacritty

You almost never need this. The combination of `xvfb-run xterm -e my-tui-app` plus `xdotool` plus `import` (ImageMagick) can produce actual PNG screenshots of a TUI for cases where Unicode rendering or font shaping matters (e.g. you're testing that a Powerline glyph rendered correctly). But this:

* Requires Xvfb, which is unnecessary because tmux already provides a virtual terminal at the *byte* level — and the byte level is what TUIs actually care about.
* Doubles the failure surface (fonts, X server, screenshot tool).
* Produces images, which means the agent burns vision tokens to read them.

Reach for this only when you genuinely need pixel rendering. For 99% of TUI driving, the tmux text snapshot is what you want.

### Other recorders worth knowing

* **`termtosvg`** — produces SVG terminal recordings, similar to asciinema but vector.
* **`script(1)` / `typescript`** — macOS and Linux's built‑in tool; see §6.

---

## 6. macOS‑specific notes

* **`script(1)` to fake a TTY.** If you have a tool that refuses to run because stdin/stdout aren't a TTY (notably Claude Code itself in some CI setups — see issue #9026), wrap it with `script`:

  ```bash
  # macOS BSD script
  script -q /dev/null my-program-that-wants-a-tty
  # Linux util-linux script
  script -qfc 'my-program-that-wants-a-tty' /dev/null
  ```
  
  This is a useful hammer when you don't want to set up tmux for a one‑shot.

* **Homebrew installs everything.** `brew install tmux asciinema vhs ttyd` covers the entire toolkit.

* **iTerm2 has Python scripting** (`iterm2` Python module + the AppleScript interface). It is theoretically possible to drive iTerm2 from Claude Code via an osascript shim. In practice nobody does this because tmux is simpler and works on Linux too.

* **Avoid iTerm2's `tmux -CC` integration mode** when running Claude Code itself in tmux: Claude Code's fullscreen renderer disables the alt‑screen / mouse tracking that `tmux -CC` assumes (per hboon.com's "Using tmux with Claude Code"). Plain tmux in iTerm2 is fine.

* **The macOS CPR/DSR leak bug** (anthropics/claude-code #17787, January 2026) caused Claude Code's own TUI to be unusable outside tmux on some macOS+terminal combos. It does not affect *your* TUI driven via tmux, but it's why "always run Claude Code inside tmux" has become near‑universal advice on macOS.

---

## 7. Headless Ubuntu / SSH server notes

**Yes, tmux + capture-pane works entirely without a display.** This is the single biggest reason the tmux approach has eaten everything else. Tmux is itself a terminal emulator running entirely in user space — it does not need X, Wayland, a framebuffer, or any GUI. On a fresh Ubuntu droplet:

```bash
sudo apt update && sudo apt install -y tmux
tmux new-session -d -s tui -x 200 -y 50 'my-tui-app'
tmux capture-pane -t tui -p
```

This works over SSH, in a Docker container, in a CI runner, anywhere a shell runs.

You do **not** need Xvfb. You would only need Xvfb if your "TUI" was actually a GUI app you were trying to screenshot. For ncurses/Bubble Tea/Ratatui/Textual/blessed‑style apps, no display server is required.

A few headless‑specific tips:

* Set `TERM=xterm-256color` (or `screen-256color` when inside tmux) so the TUI knows colors are available: `tmux new-session -d -s tui -e 'TERM=xterm-256color' -x 200 -y 50 'my-tui-app'`.
* On minimal containers you may also need `locale-gen en_US.UTF-8` and `LANG=en_US.UTF-8` so Unicode box drawing renders correctly inside tmux.
* For long‑lived servers, prefer the dedicated‑socket pattern (`tmux -L claude-tui -f /dev/null …`) so agent‑owned sessions cannot clobber any sysadmin tmux sessions.

---

## 8. Recommended recipe — copy‑pasteable

For most users on macOS or Ubuntu, the following is the path of least resistance. Put it in your `CLAUDE.md` so Claude knows the pattern, or wrap it as a skill.

### One‑time install

```bash
# macOS
brew install tmux

# Ubuntu / Debian
sudo apt install -y tmux
```

### Tell Claude how to drive your TUI (`CLAUDE.md` snippet)

````markdown
## Driving the TUI

When asked to interact with our TUI app `myapp`, use tmux on an isolated socket.
Use these exact patterns:

```bash
SOCK=claude-tui

# Start (do this once per session)
tmux -L "$SOCK" -f /dev/null new-session -d -s tui -x 200 -y 50 \
  -e 'TERM=xterm-256color' 'myapp'
sleep 0.5

# Observe (text-only — preferred)
tmux -L "$SOCK" capture-pane -t tui -p

# Observe with scrollback and ANSI colors
tmux -L "$SOCK" capture-pane -t tui -eJp -S -200

# Act — special keys
tmux -L "$SOCK" send-keys -t tui Down Down Enter
tmux -L "$SOCK" send-keys -t tui C-c                # Ctrl-C
tmux -L "$SOCK" send-keys -t tui Escape ':wq' Enter

# Act — literal text (use -l to suppress key-name interpretation)
tmux -L "$SOCK" send-keys -t tui -l 'some literal text with $vars'
tmux -L "$SOCK" send-keys -t tui Enter

# Wait for the screen to stop changing instead of fixed sleeps
prev=""; for i in 1 2 3 4 5 6 7 8 9 10; do
  cur="$(tmux -L "$SOCK" capture-pane -t tui -p)"
  [ "$cur" = "$prev" ] && [ -n "$cur" ] && break
  prev="$cur"; sleep 0.15
done

# Tear down
tmux -L "$SOCK" kill-server
```

Gotchas to obey:
- Always `sleep 0.3–0.5` after `new-session` before the first capture.
- `Enter` is a separate argument; don't append `\n` to strings.
- Set pane size explicitly with `-x` and `-y`; tmux defaults are 80×24.
- Use `-l` with `send-keys` for arbitrary literal text.
- Run cleanups in finally-style blocks to avoid leaking sessions.
````

### A tiny wrapper script that's nicer to call

Save as `~/bin/tmuxdrive` (chmod +x) and put it on PATH. Most agent skills end up with something like this.

```bash
#!/usr/bin/env bash
# Tiny TUI driver for AI agents. Isolated tmux server.
set -euo pipefail
SOCK="${TMUXDRIVE_SOCK:-claude-tui}"
T() { tmux -L "$SOCK" -f /dev/null "$@"; }

cmd="${1:-}"; shift || true
case "$cmd" in
  start)
    name="$1"; shift
    T new-session -d -s "$name" -x "${COLS:-200}" -y "${ROWS:-50}" \
      -e 'TERM=xterm-256color' "$@"
    sleep 0.4
    ;;
  send) name="$1"; shift; T send-keys -t "$name" "$@" ;;
  type) name="$1"; shift; T send-keys -t "$name" -l "$*"; T send-keys -t "$name" Enter ;;
  see)  name="$1"; T capture-pane -t "$name" -p ;;
  see-color) name="$1"; T capture-pane -t "$name" -eJp -S -200 ;;
  wait-stable)
    name="$1"; prev=""
    for _ in $(seq 1 20); do
      cur="$(T capture-pane -t "$name" -p)"
      [ "$cur" = "$prev" ] && [ -n "$cur" ] && { echo "$cur"; exit 0; }
      prev="$cur"; sleep 0.15
    done
    echo "$cur"
    ;;
  stop) name="$1"; T kill-session -t "$name" ;;
  nuke) T kill-server 2>/dev/null || true ;;
  *) echo "usage: $0 {start NAME CMD...|send NAME KEYS...|type NAME TEXT|see NAME|see-color NAME|wait-stable NAME|stop NAME|nuke}" >&2; exit 1 ;;
esac
```

Then Claude can run:

```bash
tmuxdrive start app  myapp --flag
tmuxdrive type  app  alice
tmuxdrive send  app  Down Down Enter
tmuxdrive wait-stable app
tmuxdrive see   app
tmuxdrive stop  app
```

### When to upgrade to terminalcp or another MCP

Switch to an MCP server like `terminalcp` if you want:

* Sessions that survive Claude Code restarts without you re‑attaching.
* A `since_last: true` delta‑mode read that returns only *new* bytes (this is implemented far more carefully than your own diffing of `capture-pane` output).
* A single tool with structured JSON arguments rather than shell quoting.
* Cross‑machine attach (the MCP server can run on a different host).

Install:

```bash
claude mcp add terminalcp -- npx @mariozechner/terminalcp@latest --mcp
```

Then ask Claude to use the `terminalcp` tool with actions `start`, `stdin`, `stdout`, `stream`, `stop`.

### Quick decision tree

* **TUI on macOS or Ubuntu, agent loop "observe → act → observe"** → tmux + Bash (recipe above). This is the right answer ~90% of the time.
* **Bubble Tea app you control the source of** → `teatest` golden files.
* **You want session persistence across Claude restarts** → `terminalcp` MCP.
* **Line‑oriented prompt automation, no curses** → `pexpect` script via Bash.
* **Pixel‑perfect screenshots / GIFs for review** → `vhs` tape, then read the `.txt` output.
* **TUI requires recorded asciicast for compliance/review** → `elleryfamilia/terminal-mcp` with `--record`.

---

## Summary

Claude Code's `Bash` tool does not allocate a PTY, and there is currently no first‑party way to change that (the relevant feature request, anthropics/claude-code #9881, is still open). Everything in the ecosystem in 2025–2026 — Anthropic's internal skills, obra's `superpowers-lab`, NousResearch's Hermes, Mario Zechner's terminalcp, the half‑dozen tmux MCP servers — works around that limitation by delegating the actual PTY to a sub‑process, then exposing send/capture primitives back to the agent. The simplest version of that idea, plain `tmux send-keys` + `tmux capture-pane -p` over Claude Code's regular Bash tool with an isolated socket and known pane size, is what virtually everyone settles on. Reach for an MCP server (terminalcp) or pexpect or VHS only when you have a specific reason to.
