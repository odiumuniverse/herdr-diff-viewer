# diff-viewer

> ✨ Beautiful and ⚡ BLAZINGLY FAST ✨

Git diff sidebar for [herdr](https://github.com): changed files with red/green hunks in a split pane. Click a file to jump, drag to send lines into the agent prompt. Written in Rust 🦀 btw.

## Usage

Run the `toggle` action from an agent pane (`Diff viewer: toggle git diff sidebar`).

Scope is a **union**, not a single repo: the pane's repo (or every child repo when opened from a parent directory like `~/my`), plus every repo the agent has been seen in — the viewer follows `foreground_cwd` live, and background hooks (`pane.created` / `pane.focused` / `pane.agent_status_changed`) keep tracking even while the viewer is closed. The header shows `watching N` so you always know the blast radius. Untracked files included. One broken repo never blanks the view.

Opt out of background tracking with `DIFF_TRACK=0`.

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
| Multi-repo union scope | ❌ one repo | ✅ anchor + children + everywhere the agent went |
| Live follow (1s) | ❌ snapshot | ✅ ticks while you work |
| Click-to-jump file list | ❌ | ✅ |
| Drag lines into prompt as `file:line` | ❌ | ✅ |
| Transparent theme | ❌ | ✅ 👻 |
| Speed | fast | ⚡ BLAZINGLY FAST (it's Rust 🦀) |
