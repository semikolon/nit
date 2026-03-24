# nit

AI-era dotfiles manager — git's rivet.

**nit** tracks dotfiles directly in `$HOME` via bare git. 94% of your files are plain configs that don't need source/target indirection. Edit in place, commit with `nit commit`, push with `nit push`. For the ~10 files that need per-machine templating, nit renders [Tera](https://keats.github.io/tera/) templates. For secrets, [age](https://age-encryption.org/) encryption with tiered access. For provisioning scripts, hash-based triggers that fire only when watched files change.

```
$HOME (bare git work tree — plain files tracked directly)
  │
  ├── ~/.zshrc, ~/.config/*, ...  ← edit in place, plain git
  │
  └── ~/dotfiles/                 ← project hub (subdirectory of the same repo)
        ├── templates/            ← .tmpl source files (~10)
        ├── secrets/              ← .age encrypted files (4 tiers)
        ├── scripts/              ← trigger scripts (19)
        ├── fleet.toml            ← shared config (machines, tiers, triggers)
        └── triggers.toml         ← hash-based trigger definitions
```

## Why nit?

Tools like chezmoi add a source/target layer that creates friction in concurrent AI-assisted workflows — merge conflicts, false drift, agent bypass. nit eliminates this for the 94% of files that don't need it, and provides a deliberately simple model for the 6% that do:

- **Plain files**: edit in place. `nit add`, `nit commit`, `nit push` — just git.
- **Templates**: source always wins on deploy. Drift is saved, never auto-merged. Review with `nit pick`.
- **Secrets**: age-encrypted, tiered per-machine access. Decrypt on deploy.
- **Triggers**: scripts that run when watched files change. No manual intervention.

## Key Design Decisions

**No auto-merge, ever.** Template source is the single source of truth. Target drift is saved to `~/.local/share/nit/drift/` for conscious review — never automatically incorporated. "Clean merge" ≠ "correct merge."

**Every touchpoint shows drift.** `nit add`, `nit apply`, `nit pick`, `nit commit` — all four show template drift and write per-session acks. Impossible to miss.

**Per-PPID ack system.** Each concurrent session writes its own ack file. No lock file, no contention. Cross-session acks reuse reviews when state matches.

**No interactive prompts.** All output is stderr, agent-friendly. No TTY input ever. Agents read errors, use their editor, re-run.

## Commands

| Command | Description |
|---------|-------------|
| `nit add <file>` | Stage a file (template targets redirect to source) |
| `nit add .` | Stage all modified tracked files + scan templates |
| `nit apply [file]` | Render + deploy templates locally (no commit) |
| `nit pick [file]` | Proactive drift review ("nitpick" your templates) |
| `nit pick --dismiss <file>` | Dismiss saved drift (shows diff before removing) |
| `nit commit -m "..."` | Render + deploy + ack gate + git commit + triggers |
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

## Configuration

### fleet.toml (shared, tracked in repo)

```toml
[machines.mac-mini]
ssh_host = "localhost"
role = ["dev", "primary"]

[machines.darwin]
ssh_host = "darwin"
role = ["dev", "server", "router"]
critical = true

[templates]
source_dir = "~/dotfiles/templates"

[secrets]
source_dir = "~/dotfiles/secrets"

[secrets.tiers.tier-all]
recipients = ["age1..."]
target = "~/.secrets/tier-all.env"

[secrets.tiers.tier-servers]
recipients = ["age1mac...", "age1darwin..."]
target = "~/.secrets/tier-servers.env"

[permissions]
private = ["~/.ssh/*", "~/.secrets/*"]

[exclude]
"templates/.claude/**" = { unless_role = "dev" }
```

### triggers.toml (trigger definitions)

```toml
[[trigger]]
name = "install-packages-darwin"
script = "scripts/darwin/install-packages.sh"
watch = [".Brewfile"]
os = "darwin"

[[trigger]]
name = "build-rust-hooks"
script = "scripts/build-rust-hooks.sh"
watch = [".claude/hooks/*/Cargo.toml", ".claude/hooks/*/src/**"]
role = "dev"
```

### local.toml (per-machine, NOT tracked)

```toml
machine = "mac-mini"
identity = "~/.config/nit/age-key.txt"

[git]
strategy = "bare"  # or "home"
```

## Git Strategy

**Strategy B (default):** Bare repo at `~/.local/share/nit/repo.git`, work tree = `$HOME`. No `~/.git` directory. Plain `git` from `$HOME` fails loudly — agents learn to use `nit`. Self-correcting.

**Strategy A (opt-in):** Regular `~/.git` repo with `GIT_CEILING_DIRECTORIES` to prevent walk-up. Agents use plain `git`. Set `strategy = "home"` in `local.toml`.

## The Name

Swedish: **nit** = rivet (permanent metal fastener).
English: meticulous about details.

`git` tracks. `nit` fastens. Three letters each. Terminal-native pair.

And yes — `nit pick` is the command that nitpicks your templates for drift.

## Installation

```bash
cargo install nit
```

Or download pre-built binaries from [GitHub Releases](https://github.com/semikolon/nit/releases).

## Status

**Work in progress.** Core CLI skeleton and config loading are implemented. Template rendering, encryption, triggers, and the ack system are coming next.

## License

MIT
