# diff-viewer

Git diff sidebar for [herdr](https://github.com): changed files with red/green hunks in a split pane. Click a file to jump, drag to send lines into the agent prompt.

## Usage

Run the `toggle` action from an agent pane (`Diff viewer: toggle git diff sidebar`). Shows `git status` changes of the pane's repo (untracked included); `t` collapses tests/generated.

Keys: `q` quit · `r` refresh · `t` tests · `j/k`, arrows, wheel scroll · `g/G` top/bottom · drag sends `file:line` + text to the agent.

## Theme

```sh
export DIFF_THEME=tokyo-night  # then re-toggle
```

`dark` / `light` follow claude-code themes, or any name from `./target/release/diff-viewer themes` (catppuccin, dracula, nord, gruvbox, one-dark/light, solarized, kanagawa, rose-pine, vesper, tokyo-night…). Unset = auto-detect from terminal background.
