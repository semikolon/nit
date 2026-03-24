# nit

AI-era dotfiles manager — git's rivet.

**nit** tracks dotfiles directly in `$HOME` via bare git. Most dotfiles are plain configs that don't need source/target indirection — edit in place, commit with `nit commit`, push with `nit push`. For the handful of files that need per-machine templating, nit renders [Tera](https://keats.github.io/tera/) templates. For secrets, [age](https://age-encryption.org/) encryption with tiered access. For provisioning, hash-based triggers that fire only when watched files change.

```
$HOME (bare git work tree — plain files tracked directly)
  │
  ├── ~/.zshrc, ~/.config/*, ...  ← edit in place, plain git
  │
  └── ~/dotfiles/                 ← project hub (subdirectory of the same repo)
        ├── templates/            ← .tmpl source files
        ├── secrets/              ← .age encrypted files (tiered)
        ├── scripts/              ← trigger scripts
        ├── fleet.toml            ← shared config (machines, tiers, triggers)
        └── triggers.toml         ← hash-based trigger definitions
```

## Why nit?

Existing dotfile managers fall into two camps:

1. **Bare git wrappers** (yadm, dotbare) — great for plain files, but no templates, no secrets, no triggers. You outgrow them when you manage multiple machines.

2. **Source/target managers** (chezmoi, dotter) — powerful templating and encryption, but every file goes through a source → target pipeline. This adds overhead for the majority of files that don't need it, and creates friction when multiple tools or sessions edit the same files concurrently.

nit takes a different approach: **plain git for plain files, nit for everything else.**

- **Plain files**: edit in place. `nit add`, `nit commit`, `nit push` — just git with the right `--git-dir` flags.
- **Templates**: Tera (Jinja2-like) rendering with per-machine variables. Source always wins on deploy. Drift is saved for review, never auto-merged.
- **Secrets**: age-encrypted with tiered per-machine access (e.g., production secrets only decrypt on servers).
- **Triggers**: provisioning scripts that run automatically when their watched files change — package installation, service reload, build tasks.

### Designed for concurrent AI workflows

nit is built for environments where multiple agents or sessions edit dotfiles simultaneously:

- **Per-PPID ack system**: each session writes its own review state. No lock file, no contention.
- **No auto-merge**: template drift is saved and shown at every touchpoint — `nit add`, `nit apply`, `nit pick`, `nit commit`. Impossible to miss.
- **No interactive prompts**: all output is stderr. No TTY input, ever. Agents read errors, use their editor, re-run.
- **Smart `nit add`**: detects template targets and redirects to staging the source file — agents don't need to know the template architecture.

## Commands

| Command | Description |
|---------|-------------|
| `nit add <file>` | Stage a file (template targets redirect to source) |
| `nit add .` | Stage all modified tracked files + scan templates |
| `nit apply [file]` | Render + deploy templates locally (no commit) |
| `nit pick [file]` | Review template drift ("nitpick" your configs) |
| `nit pick --dismiss <file>` | Dismiss saved drift (shows diff before removing) |
| `nit commit -m "..."` | Render + deploy + ack check + git commit + triggers |
| `nit update` | Pull + render + deploy + triggers (fleet sync) |
| `nit update --safe` | Same, but skip service-restarting triggers |
| `nit status` | One-line summary |
| `nit list` | Inventory of templates, secrets, triggers |
| `nit encrypt <file>` | Add to age-encrypted secrets |
| `nit decrypt <file>` | Decrypt to stdout |
| `nit rekey` | Re-encrypt all secrets with current recipients |
| `nit run <name>` | Manually run a trigger |
| `nit bootstrap <url>` | Clone repo + configure + initial deploy |
| `nit push/pull/log/...` | Falls through to git with correct flags |

## Quick Start

```bash
# Install
cargo install nit

# Bootstrap on a new machine (clones repo, sets up identity)
nit bootstrap git@github.com:you/dotfiles.git

# Day-to-day: edit files in place, commit, push
vim ~/.zshrc
nit add ~/.zshrc
nit commit -m "update zsh config"
nit push

# Template workflow: edit source, preview, commit
vim ~/dotfiles/templates/.zshenv.tmpl
nit pick           # review what will change
nit commit -m "add sccache config per-machine"

# Fleet sync (on remote machines via SSH or cron)
nit update         # pull + render + decrypt + triggers
nit update --safe  # same, but skip service-restarting triggers
```

## Configuration

### fleet.toml (shared, tracked in repo)

Defines your fleet of machines, templates, secrets, and exclusion rules. Read by both nit and your fleet orchestrator.

```toml
[machines.laptop]
ssh_host = "laptop"
role = ["dev"]

[machines.server]
ssh_host = "server"
role = ["dev", "server"]
critical = true

[machines.router]
ssh_host = "router"
role = ["router"]

[templates]
source_dir = "~/dotfiles/templates"

[secrets]
source_dir = "~/dotfiles/secrets"

[secrets.tiers.tier-all]
recipients = ["age1laptop...", "age1server...", "age1router..."]
target = "~/.secrets/tier-all.env"

[secrets.tiers.tier-servers]
recipients = ["age1laptop...", "age1server..."]
target = "~/.secrets/tier-servers.env"

[permissions]
private = ["~/.ssh/*", "~/.secrets/*"]

[exclude]
"templates/.claude/**" = { unless_role = "dev" }
"secrets/tier-servers*" = { unless_role = "server" }
```

### triggers.toml

Declarative trigger definitions. Each trigger specifies a script, the files it watches, and optional OS/role filters.

```toml
[[trigger]]
name = "install-packages"
script = "scripts/install-packages.sh"
watch = [".Brewfile"]
os = "darwin"

[[trigger]]
name = "reload-services"
script = "scripts/reload-services.sh"
watch = ["Library/LaunchAgents/*.plist"]
os = "darwin"

[[trigger]]
name = "build-tools"
script = "scripts/build-tools.sh"
watch = ["tools/*/Cargo.toml", "tools/*/src/**"]
role = "dev"
```

### local.toml (per-machine, NOT tracked)

Generated by `nit bootstrap`. Just says "who am I?"

```toml
machine = "laptop"
identity = "~/.config/nit/age-key.txt"

[git]
strategy = "bare"  # default; or "home" for ~/.git
```

## Git Strategy

**Bare repo (default):** `~/.local/share/nit/repo.git` with `$HOME` as work tree. No `~/.git` directory exists. Plain `git` from `$HOME` fails with "not a git repository" — loud and self-correcting.

**Home repo (opt-in):** Regular `~/.git` with `GIT_CEILING_DIRECTORIES` to prevent subdirectory walk-up. Use plain `git` directly. Set `strategy = "home"` in `local.toml`.

## How Templates Work

Templates live in `~/dotfiles/templates/` and mirror the target directory structure:

```
templates/.zshenv.tmpl              → ~/.zshenv
templates/.config/foo/config.tmpl   → ~/.config/foo/config
templates/Library/LaunchAgents/x.plist.tmpl → ~/Library/LaunchAgents/x.plist
```

Rendered files include a warning comment (`# Managed by nit` or `<!-- Managed by nit -->`). Template variables include `hostname`, `os`, `arch`, `role`, and custom data from `fleet.toml`.

### Drift handling

When a rendered target has been edited directly:

1. **Source always wins on deploy.** The edit is overwritten.
2. **Drift is saved** to `~/.local/share/nit/drift/` for review.
3. **`nit pick`** shows the drift. You decide: edit the template source to incorporate it, or dismiss it.
4. **Drift persists** until explicitly dismissed with `nit pick --dismiss`.

No auto-merge, ever. "Clean merge" ≠ "correct merge."

## The Name

Swedish: **nit** = rivet (permanent metal fastener).
English: meticulous about details.

`git` tracks. `nit` fastens. Three letters each.

And yes — `nit pick` is the command that nitpicks your templates for drift.

## Status

**Work in progress.** Core CLI skeleton, config loading, template discovery, and smart `nit add` are implemented. Template rendering, encryption, triggers, and the ack system are coming next.

See [CHANGELOG.md](CHANGELOG.md) for progress updates.

## License

MIT
