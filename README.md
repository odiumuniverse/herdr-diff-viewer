# diff-viewer

[![CI](https://github.com/odiumuniverse/herdr-diff-viewer/actions/workflows/ci.yml/badge.svg)](https://github.com/odiumuniverse/herdr-diff-viewer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-macos%20%7C%20linux-lightgrey)
![Rust](https://img.shields.io/badge/rust-stable-orange?logo=rust)
![Themes](https://img.shields.io/badge/themes-23-blueviolet)

> ✨ Beautiful and ⚡ BLAZINGLY FAST ✨

Git diff sidebar for [herdr](https://github.com): changed files with red/green hunks in a split pane. Click a file to jump, drag to send lines into the agent prompt. Written in Rust 🦀 btw.

![diff-viewer screenshot](assets/screenshot.png)

## Usage

Run the `toggle` action from an agent pane (`Diff viewer: toggle git diff sidebar`).

Scope is **per agent session**, never per tab or pane cwd. Two sources feed it:

**1. Agent journals (exact).** The viewer reads the session's own transcript and
adopts every repo the session edited — no cwd assumptions, works even when the
agent writes to a repo it was not launched in.

| agent | journal | what counts |
|---|---|---|
| claude | `~/.claude/projects/<slug>/<session>.jsonl` | Edit/Write/MultiEdit/NotebookEdit + subagent journals |
| opencode | `~/.local/share/opencode/opencode.db` | `edit`/`write` + `apply_patch` patch text, plus shell `workdir` (gated); legacy `part` and v2 `session_message` merged, subagent sessions included |
| kilo | `~/.local/share/kilo/kilo.db` | same as opencode |
| codex | `~/.codex/sessions/**.jsonl` | `apply_patch` / `patch_apply_end` changes |
| gemini | `~/.gemini/tmp/<project>/chats/session-*` | `write_file`/`replace` tool calls |
| qwen | `~/.qwen/projects/<slug>/chats/<session>.jsonl` | `edit`/`write_file` tool calls |
| pi / omp | `~/.pi/…`, `~/.omp/agent/sessions/…` | `edit`/`write` tool calls |
| antigravity | `…/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl` | `write_to_file`/`replace_file_content` targets |
| cursor | `~/.cursor/projects/<slug>/agent-transcripts/<conv>.jsonl` + `ai-tracking.db` | best-effort (no cwd in store) |
| aider | `<repo>/.aider.chat.history.md` | `> Applied edit to <path>` |

**2. Shell workdirs (gated).** opencode and kilo record the working directory of
every shell command; those directories join the scope as candidates and are
adopted only once their working tree changes, so a repo the session merely reads
stays out.

**3. Process fallback (signature-gated).** For agents without a journal (and for
shell-only edits), a repo is adopted when a new process of the session's own
tree runs with its cwd inside it *and* its working tree changes while the
session is alive — so a dirty launch repo that the session never touched stays
invisible. Sources marked best-effort above use the same gate. Agents that
multiplex every session behind one shared daemon (opencode, kilo) never use the
fallback: their pane's process tree spans unrelated sessions, so it is not
walked.

The pane's own cwd repo is never assumed. Once a repo is in scope, all of its
uncommitted changes are shown, whoever made them. Session id or agent change
resets the scope; a new session starts empty; a confirmed pane close deletes it.
The viewer follows its own agent pane live — neighbour panes never leak in. The
header shows `watching N` so you always know the blast radius. Untracked files
included. One broken repo never blanks the view.

Debug a journal reader from any shell:

```sh
diff-viewer journals claude "" <session-id>   # cwd + paths the viewer would adopt
```

Limitations: shell edits that finish between samples and agents with neither a
journal nor visible child processes are not detected; cursor/aider/antigravity
blob sources are heuristic and always gated.

Toggle resyncs pane sizes after open/close (`resize --amount 0` is a no-op that still syncs pty winsizes), so neither the viewer nor the agent renders clipped until the next click.

## Theme

Click `◑` in the header or press `T` for a theme menu — applies instantly, persists across toggles. `j`/`k` + `Enter` to pick, `Esc` to close.

```sh
cargo install --path .     # once: `diff-viewer` in ~/.cargo/bin
diff-viewer themes         # list
diff-viewer theme NAME     # set from any shell, picked up live, no re-toggle
diff-viewer theme auto     # back to claude-code dark/light auto-detect
```

`transparent` = same syntax colors as `claude-code-dark`, but the gray base stays unpainted (your terminal background / blur shows through) with juicier reds and greens. Also available: `dark` / `light` (follow claude-code themes), or any name from `diff-viewer themes` (catppuccin, dracula, nord, gruvbox, one-dark/light, solarized, kanagawa, rose-pine, vesper, tokyo-night…). Unset = auto-detect from terminal background.

## Why not Claude Code `/diff`?

| | `/diff` | diff-viewer |
|---|---|---|
| Multi-repo scope | ❌ one repo | ✅ own session's repos only |
| Live follow (1s) | ❌ snapshot | ✅ ticks while you work |
| Click-to-jump file list | ❌ | ✅ |
| Drag lines into prompt as `file:line` | ❌ | ✅ |
| Transparent theme | ❌ | ✅ 👻 |
| Speed | fast | ⚡ BLAZINGLY FAST (it's Rust 🦀) |
