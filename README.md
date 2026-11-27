# mcr — Merge Conflict Resolver

A keyboard-first, three-pane Git merge TUI written in Rust. The incoming
branch is on the left, your branch on the right, and the merged result in the
middle. Resolve conflicts hunk by hunk or line by line, mix lines from both
sides, and undo anything — with editor-grade syntax highlighting throughout.

![mcr resolving a Python merge in the default Tokyo Night theme: source branch, merged result and target branch side by side](docs/screenshot.png)

## Install

```sh
cargo install --path .        # installs the `mcr` binary (Rust 1.82+, git on $PATH)
```

## Usage

```sh
mcr                         # resolve the conflicts of the merge / rebase / cherry-pick in progress
mcr merge feature/x         # run `git merge feature/x`, then resolve what conflicts
mcr file LOCAL REMOTE --base BASE -o MERGED     # merge three plain files
mcr print-config            # print a commented config file with every default
mcr -C path/to/repo --theme tokyonight-storm --config ./mcr.toml
```

As a `git mergetool`:

```ini
[merge]
    tool = mcr
[mergetool "mcr"]
    cmd = mcr file "$LOCAL" "$REMOTE" --base "$BASE" -o "$MERGED"
    trustExitCode = true
```

### Workflow

1. `mcr` opens the first conflicted file with the cursor on the first
   unresolved hunk. Changes only one branch made are already merged, exactly
   as git would merge them (and can still be overridden).
2. Resolve each hunk — whole (`s`, `t`, `b`, `d`), line by line (`space`,
   `x`), or by hand (`e`).
3. `w` writes the file. Once nothing is unresolved it is also `git add`ed.
4. `] f` moves to the next file; `q` quits. Finish with `git commit`.

### Saving and Resuming

`w` writes the file and remembers every decision you made in it. Quit
whenever you like: the next `mcr` in the same merge picks up where you
saved — accepted and rejected lines, their order, hand edits and the cursor
position. Files you already finished stay in the tab row with a `✓`, so you
can review or change them until you commit.

Progress is kept under `.git/mcr-session/` and removed once the merge is
over. It is only resumed while it still describes reality: if the file on
disk is no longer what `mcr` saved (you edited it elsewhere, or restarted the
merge), that file starts fresh. The undo history is not carried across runs.

## Look

The panes have no borders: each has a header strip, underlined in its color
while it has the focus. Only the current line is set apart, with its own
background, across all three panes.

Icons come from a [Nerd Font](https://www.nerdfonts.com) (any patched font,
e.g. JetBrainsMono Nerd Font). The typeface itself is your terminal's — a
terminal program cannot choose it. Without a Nerd Font set
`ui.icons = "unicode"` (plain symbols, shown in the tables below) or
`"ascii"`.

### Syntax Highlighting

Code is parsed with [tree-sitter](https://tree-sitter.github.io) grammars and
colored the way nvim-treesitter does it: keywords, functions, types,
components, properties, parameters, strings, numbers, operators, comments
and more each get their own color from the theme. Highlighting stays on over
diff and conflict backgrounds; rejected lines are dimmed and struck through
instead.

Supported: Python, Rust, JavaScript / JSX, TypeScript / TSX, Go, JSON, TOML,
YAML, Markdown, Bash, HTML and CSS, recognized by file name. Other files —
and files over 1 MB per side — are shown plain. `ui.syntax = false` turns
highlighting off, `ui.italic = false` the italics of comments and
control-flow keywords.

### Themes

A theme colors the whole interface *and* the code. Built in:

| Theme | |
| --- | --- |
| `tokyonight` | Tokyo Night — the default |
| `tokyonight-storm` | the same on a slate background |
| `tokyonight-day` | the light variant |
| `dark`, `light` | Material |

Choose one with `ui.theme` or `--theme`, and switch while running with
`ctrl-t`. `theme = "auto"` picks `tokyonight`, or `tokyonight-day` on a light
terminal.

| Tokyo Night (default) | Tokyo Night Day |
| --- | --- |
| ![A TSX merge in the Tokyo Night theme](docs/screenshot-tsx.png) | ![The same merge in the light Tokyo Night Day theme](docs/screenshot-day.png) |

**Adding a theme.** Drop a file into `themes/` next to your config file; its
name is the theme's name:

```sh
mkdir -p ~/.config/mcr/themes
cp themes/catppuccin-mocha.toml ~/.config/mcr/themes/
mcr --theme catppuccin-mocha
```

[`themes/catppuccin-mocha.toml`](themes/catppuccin-mocha.toml) is a complete,
commented theme to copy from. Every key is optional — what a theme leaves out
comes from its `base` — so adjusting a built-in theme takes a few lines. The
same keys also work inline in `config.toml` (see [Configuration](#configuration)).

![The same merge with the Catppuccin Mocha theme file installed](docs/screenshot-catppuccin.png)

## Reading the Screen

All three panes share one cursor row and scroll together; shorter sides are
padded so a hunk always lines up across the panes.

| Marker | Meaning |
| --- | --- |
| `✓` | accepted — the line is in the result |
| `✗` (struck through) | rejected — the line is left out |
| `?` | unresolved — still needs a decision |
| `~` | modified — the hunk was edited by hand |
| `◀` / `▶` | result line taken from the source / target branch |
| `▎` | the hunk under the cursor |

Backgrounds are soft tints: orange for conflicts, green / teal / red for
lines added / changed / removed relative to the common ancestor, cyan and
purple in the result for lines that came from the source and target branch.
Above the panes, the title bar shows the merge and a progress bar of the
hunks resolved in this file; the tab row lists every conflicted file with
the number of hunks still open (`✓` once it has none). The status bar names
the focused pane and the state of the hunk under the cursor. Setting
`ui.minimap = true` adds a column at the far right that maps the whole file
(orange: unresolved, green: resolved, grey: merged automatically).

## Keys

Defaults; every one of them can be changed (see below). `?` shows the live
list inside the app.

| Keys | Action |
| --- | --- |
| `j` `k` / arrows | move by line |
| `ctrl-d` `ctrl-u`, `ctrl-f` `ctrl-b` | half page, full page |
| `g g`, `G` | first / last line |
| `J`, `K` | next / previous change |
| `n` `N` (or `] c` `[ c`) | next / previous **unresolved** hunk |
| `h` `l` / `tab` | move focus between the panes |
| `H`, `L` | scroll text horizontally |
| `space` | accept the line under the cursor (again: undecide) |
| `x` | reject the line under the cursor (again: undecide) |
| `enter` | take the focused side's hunk; in the result pane: finish a line-by-line pick |
| `s`, `t` | take the source / target hunk |
| `b`, `B` | take both: source first / target first |
| `d` | take neither side |
| `c` | reject the hunk's pending lines, keep the accepted ones |
| `r` | reset the hunk |
| `e` | edit the hunk's result in `$EDITOR` |
| `S`, `T` | take source / target for every unresolved hunk |
| `u` `z`, `ctrl-r` `Z` | undo, redo |
| `w` / `ctrl-s`, `W` | save the file, save all files |
| `] f` `[ f`, `f` | next / previous file, file picker |
| `ctrl-t` | switch theme |
| `v` | columns ↔ stacked layout |
| `+` `-` `=` | widen / narrow the result pane, reset the layout |
| `?`, `q` | help, quit |

The mouse works too: click to place the cursor, wheel to scroll.

### Combining Lines From Both Sides

Accepted lines appear in the result in the order you accept them, except
that lines of the same side always keep their file order. So `space` on a
source line, then on a target line, then on another source line gives
source / target / source. A hunk counts as resolved once every one of its
lines is accepted or rejected — `c` (or `enter` in the result pane) rejects
whatever is still pending.

## What mcr Guarantees

* **Nothing is discarded silently.** A hunk with undecided lines is written
  as a standard conflict block (`<<<<<<<` / `|||||||` / `=======` /
  `>>>>>>>`) containing both sides and the ancestor, and the file is *not*
  staged.
* **The index is only changed through git** — `git add` when a file is fully
  resolved, `git update-index --unresolve` if you later reopen it. mcr never
  commits.
* **Nothing is written until you save**, and quitting with unsaved work asks
  first. Saved work is resumed on the next run, never silently replaced.
* The working-tree file as it was before the first save is kept under
  `.git/mcr-backup/`.
* Files are written byte-exact: CRLF stays CRLF, a missing final newline
  stays missing.
* Conflicts that are not line-mergeable text — delete/modify, binary files,
  symlinks, submodules, non-UTF-8 — are listed and left untouched.

## Configuration

`$MCR_CONFIG`, or `$XDG_CONFIG_HOME/mcr/config.toml` (`~/.config/mcr/config.toml`).
[`config.example.toml`](config.example.toml) lists every option with its
default; `mcr print-config` regenerates it.

```toml
[ui]
theme = "midnight"          # "auto", a built-in theme, or one defined below
icons = "nerd"              # or "unicode", "ascii"
layout = "stacked"          # or "columns"
panel_ratio = [2, 3, 2]     # source, result, target
auto_advance = true         # jump to the next conflict after resolving one

[git]
auto_stage = false          # do not `git add` resolved files

[keys]
take_source = "<"           # replaces the default binding of that action
take_target = ">"
save = ["w", "ctrl-s", "f2"]
goto_top = "g g"            # multi-key sequences
toggle_theme = []           # unbind

[themes.midnight]
base = "tokyonight"         # start from any built-in theme
bg = "#0f111a"
accent = "#84ffff"
conflict = "#ff5370"
tint = 0.22                 # opacity of diff / conflict backgrounds

[themes.midnight.syntax]    # colors of the code
keyword = "#c792ea"
string = "#c3e88d"
comment = "#546e7a"
```

Keys are written as typed — `j`, `G`, `space`, `enter`, `ctrl-d`, `alt-x`,
`shift-tab`, `f5` — or in Vim notation (`<c-d>`, `<space>`). Binding a key
to an action takes it away from the default action that used it.

Interface colors: `bg` `surface` `fg` `muted` `gutter` `cursor` `border`
`accent` `source` `target` `added` `removed` `modified` `conflict` `accepted`
`rejected` `unresolved`.

Syntax colors: `keyword` `keyword_control` `function` `function_builtin`
`type` `type_builtin` `constant` `string` `escape` `comment` `property`
`parameter` `variable` `variable_builtin` `operator` `punctuation` `tag`
`attribute`.

Terminals have no alpha channel, so the "translucent" backgrounds are the
accent color blended into the background at `tint` opacity. Without 24-bit
color (`$COLORTERM`), colors fall back to the nearest of the 256-color
palette; `ui.icons = "ascii"` keeps markers and borders to plain ASCII.

## Architecture

```
src/
  core/      merge engine — no I/O, no UI
    diff3.rs     three-way line diff → hunks
    session.rs   per-line resolution state, operations (command pattern), output,
                 snapshots for resuming
    history.rs   undo / redo (mementos of hunk state)
  git/       where inputs come from and results go: the MergeBackend trait,
             implemented for a repository (via the git CLI) and for plain files;
             saved progress; blame parsing (not shown in the interface at present)
  syntax.rs  tree-sitter highlighting into theme-independent tokens
  config/    config file, keymap (multi-key sequences), themes
  action.rs  the commands a key can trigger
  app/       application state: events in, state changes and effects out
  ui/        rendering — a pure function of the app state
  tui.rs     terminal event loop, $EDITOR hand-off
```

Each layer depends only on the ones above it in this list. The app talks to
git through the `MergeBackend` trait and never touches the terminal itself,
so the whole interaction model runs in tests without a TTY.

## Development

```sh
cargo test                    # unit, git-integration and end-to-end tests
cargo clippy --all-targets
cargo fmt --check
```

The integration tests build real repositories in temporary directories
(merge, rebase, add/add, delete/modify, odd file names) and drive the app
with key events against an in-memory terminal.

The screenshots in `docs/` are captures of the real program on a 150×40
terminal with JetBrainsMono Nerd Font.

## Limitations

* Line-based: no word-level diff inside a line.
* Highlighting is syntactic (tree-sitter), not semantic: there is no language
  server, so a name's color depends on its position in the code, not its type.
* The grammars are compiled in, which makes the binary about 11 MB.
* Free-form editing happens in `$EDITOR`, one hunk at a time.
* `theme = "auto"` relies on `$COLORFGBG`; without it, dark is assumed.
* The default icons need a Nerd Font; see `ui.icons`.
