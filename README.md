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

2. **Source/target managers** (chezmoi, dotter) — powerful templating and encryption, but every file goes through a source → target pipeline. This creates friction when multiple sessions edit the same files concurrently, and adds overhead for the majority of files that don't need the indirection.

nit takes a different approach: **plain git for plain files, nit for everything else.**

- **Plain files**: edit in place. `nit add`, `nit commit`, `nit push` — just git with the right `--git-dir` flags.
- **Templates**: Tera (Jinja2-like) rendering with per-machine variables. Source always wins on deploy. Drift is saved for review, never auto-merged.
- **Secrets**: age-encrypted with tiered per-machine access (e.g., production secrets only decrypt on servers).
- **Triggers**: provisioning scripts that run automatically when their watched files change — package installation, service reload, build tasks.

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
# Install from source
cargo install --git https://github.com/semikolon/nit.git

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

## How Templates Work

Templates live in `~/dotfiles/templates/` and mirror the target directory structure:

```
templates/.zshenv.tmpl              → ~/.zshenv
templates/.config/foo/config.tmpl   → ~/.config/foo/config
templates/Library/LaunchAgents/x.plist.tmpl → ~/Library/LaunchAgents/x.plist
```

Rendered files include a warning comment (`# Managed by nit` or `<!-- Managed by nit -->`). Template variables include `hostname`, `os`, `arch`, `role`, and custom data from `fleet.toml`.

### Why source always wins

In practice, drift (edits to rendered targets) is roughly 50/50 junk vs valuable. But the failure modes are asymmetric:

- **Wrongly discarding drift** → recoverable. Drift is saved to `~/.local/share/nit/drift/`, recoverable via `nit pick` at any time.
- **Wrongly merging junk** → contamination. Silent, hard to detect, and triggers may fire with bad config.

The safer default is the one with the recoverable failure mode. Source wins. Drift persists until you consciously address it.

### Drift handling walkthrough

**Scenario:** You SSH into a server and hotfix `~/.zshenv` directly (a rendered template target). Later, `nit update` pulls new changes.

```
$ nit update

  ⚠ Skipped ~/.zshenv — target has local drift
  Deployed 9/10 templates. 1 skipped (drift).
```

On fleet machines, `nit update` **skips** drifted targets entirely — your hotfix is preserved. On your dev machine, `nit commit` would overwrite the drift but save it first. Either way, nothing is lost.

To review what drifted:

```
$ nit pick

  ⚠ Drift is NEVER auto-merged. Source always wins on deploy.
  Actions for each drifted file:
    → Do nothing:          source wins on next nit commit (drift saved, recoverable)
    → Edit template source: incorporate changes you want, then nit commit
    → nit pick --dismiss:  acknowledge as junk, remove from drift permanently

  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  Drift in 1 of 10 templates:

    .zshenv — target has content not in template source:
      + export EMERGENCY_FIX=1

  9 templates clean.
```

Three paths forward:
1. **Incorporate it**: edit `templates/.zshenv.tmpl` to add the fix, then `nit commit`.
2. **Dismiss it**: `nit pick --dismiss .zshenv` — shows the diff being dropped, removes from drift.
3. **Do nothing**: source wins on next deploy. Drift stays saved. Safe default.

### Smart `nit add`

`nit add` detects template targets and does the right thing:

```bash
# Agent edits a rendered template target directly:
nit add ~/.zshenv
# nit: ~/.zshenv is a template target → staging source templates/.zshenv.tmpl
# nit: drift check for ~/.zshenv (full review with `nit pick`)

# Bulk staging works too:
nit add .
# Stages all modified tracked files via git
# Additionally scans all templates for drift
```

Agents don't need to understand the template architecture. They edit files, run `nit add`, and nit routes everything correctly.

## The Ack System: Why It Works This Way

The ack (acknowledgment) system is the commit gate for template changes. It exists because templates have a source/target split where drift can go unnoticed — unlike plain files where git's merge conflict is the only gate needed.

### The problem it solves

Without a gate, an agent could:
1. Edit a template source
2. Run `nit commit`
3. Silently overwrite a valuable target hotfix it never saw

The ack system ensures **every template change is reviewed before commit**, without requiring a separate review step. It's structural enforcement — the tool refuses, not the process.

### How it works

Each session (identified by PPID) writes its own ack file:

```
~/.local/share/nit/acks/
  12345.json    ← Session A's acks
  67890.json    ← Session B's acks
```

Each ack entry records what was on disk when reviewed:

```json
{
  ".zshenv": {
    "target_hash": "sha256:abc...",
    "rendered_hash": "sha256:xyz...",
    "timestamp": "2026-03-24T10:30:00Z"
  }
}
```

- `target_hash`: what was on disk (the target) when reviewed
- `rendered_hash`: what nit would render from source at review time
- Together, these detect both target drift AND source changes since review

### Per-PPID: why not a global lock?

A global lock file creates contention between concurrent sessions. Per-PPID acks mean:
- Each session writes only its own file (atomic rename, no concurrent write risk)
- Each session reads all ack files (safe — writers use temp-then-rename)
- Dead sessions are pruned (`kill -0` check on each PPID)
- No lock, no contention, no deadlocks. Scales to any number of concurrent sessions.

### The commit flow

When you run `nit commit` with staged template sources:

1. **Own ack exists and hashes match current state?** → Proceed. You've reviewed this.
2. **Own ack exists but hashes changed?** → Block: "target/source changed since review — re-review."
3. **No own ack, but another session reviewed the same state?** → Proceed in one run. Same content, already reviewed. Drift shown inline as informational note.
4. **No ack at all?** → Show drift inline, write ack, refuse. Second `nit commit` proceeds.

**Plain-file-only commits skip all ack checks** — zero friction for the common case.

### Why cross-session acks proceed in one run

When Session B commits and finds that Session A already reviewed the identical state (same `target_hash` and `rendered_hash`), it proceeds immediately. The alternative — always requiring two runs — would add pure friction with no safety benefit, since the content is identical. Session B still sees the drift inline.

### Every touchpoint writes acks

`nit add`, `nit apply`, `nit pick`, and `nit commit` — all four show drift and write acks. There is no separate "review" step to forget. Interacting with templates at all means reviewing them. This is by design: gates that require discipline fail; gates that resolve themselves protect.

## Fleet Sync

### `nit update` — fleet machines

`nit update` is what remote machines run (via cron, SSH, or your fleet orchestrator):

```bash
nit update         # git pull + render + decrypt + triggers
nit update --safe  # same, but skip service-restarting triggers (for production)
```

Key behavior: **drifted template targets are SKIPPED, not overwritten**. If an agent SSH'd into a server and hotfixed a config, `nit update` preserves that fix and reports it. This is the opposite of `nit commit` (which overwrites drift on your dev machine but saves it). The reasoning: fleet machines may have locally-applied fixes that haven't been promoted to source yet. Clobbering them would cause outages.

### `fleet.toml` — shared fleet inventory

`fleet.toml` is the single source of truth for your machine inventory. It's designed to be read by both nit and your fleet orchestrator (such as [hemma](https://github.com/semikolon/hemma), a Just-based fleet provisioning tool that shares this config file).

```toml
[machines.laptop]
ssh_host = "laptop"
role = ["dev"]

[machines.server]
ssh_host = "server"
role = ["dev", "server"]
critical = true    # excluded from bulk operations by default

[machines.router]
ssh_host = "router"
role = ["router"]
```

`role` is an array of strings. Triggers, template exclusions, and secret tiers all filter on role — a machine with `role = ["dev", "server"]` gets both dev-only and server-only resources.

Marking a machine as `critical = true` is a signal to fleet orchestrators that it should be excluded from automated bulk operations (like `hemma apply --all`) and require explicit targeting.

## Secrets

age-encrypted with tiered per-machine access:

```toml
[secrets.tiers.tier-all]
recipients = ["age1laptop...", "age1server...", "age1router..."]
target = "~/.secrets/tier-all.env"

[secrets.tiers.tier-servers]
recipients = ["age1laptop...", "age1server..."]
target = "~/.secrets/tier-servers.env"
```

- `nit commit` / `nit update` decrypt authorized secrets to their target paths (0600 permissions).
- Machines whose key isn't in the recipients list skip that tier silently — no error, no deployment.
- `nit encrypt <file>` encrypts and stages. `nit decrypt <file>` decrypts to stdout for inspection.
- `nit rekey` re-encrypts everything when you add or remove a machine's key.

Ciphertext lives in git on all machines. Only machines with the right key can decrypt.

## Triggers

Hash-based provisioning scripts defined in `triggers.toml`:

```toml
[[trigger]]
name = "install-packages"
script = "scripts/install-packages.sh"
watch = [".Brewfile"]
os = "darwin"
```

- `nit commit` hashes each trigger's watched files against stored state. Changed → run the script. Unchanged → skip.
- `os` and `role` filters ensure triggers only run on applicable machines.
- **Triggers skip drifted files** — if a template target had drift that was overwritten, the trigger for that file is suppressed. Drift might be junk; firing a trigger with junk config is worse than skipping it.
- `nit run <name>` manually executes a trigger regardless of hash state.
- State stored in `~/.local/share/nit/state.json`. Logs in `~/.local/share/nit/logs/`.

## Git Strategy

**Bare repo (default):** `~/.local/share/nit/repo.git` with `$HOME` as work tree. No `~/.git` directory exists. Plain `git` from `$HOME` fails with "not a git repository" — loud and self-correcting. Agents learn to use `nit` because `git` doesn't work.

**Home repo (opt-in):** Regular `~/.git` with `GIT_CEILING_DIRECTORIES` to prevent subdirectory walk-up. Agents can use plain `git` directly. Set `strategy = "home"` in `local.toml`.

The bare repo strategy avoids a class of problems where tools walk up from subdirectories and find `~/.git` unexpectedly (build tools, code indexers, context detectors). Having no `~/.git` makes this impossible.

## Configuration Reference

### fleet.toml (shared, tracked in repo)

```toml
[machines.laptop]
ssh_host = "laptop"              # SSH host alias (must match ~/.ssh/config)
role = ["dev"]                   # roles for filtering triggers, secrets, exclusions
critical = false                 # default; true = exclude from bulk fleet operations

[templates]
source_dir = "~/dotfiles/templates"

[secrets]
source_dir = "~/dotfiles/secrets"

[secrets.tiers.tier-all]
recipients = ["age1...", "age1..."]
target = "~/.secrets/tier-all.env"

[permissions]
private = ["~/.ssh/*", "~/.secrets/*", "~/.config/nit/age-key.txt"]

[exclude]
"templates/.special/**" = { unless_role = "dev" }
"secrets/tier-servers*" = { unless_role = "server" }

[sync]
command = "nit update"
schedule = "03:00"
idle_gated = true

[sync.overrides.server]
strategy = "safe"
```

### triggers.toml (trigger definitions, tracked in repo)

```toml
[[trigger]]
name = "install-packages"
script = "scripts/install-packages.sh"
watch = [".Brewfile"]
os = "darwin"

[[trigger]]
name = "build-tools"
script = "scripts/build-tools.sh"
watch = ["tools/*/Cargo.toml", "tools/*/src/**"]
role = "dev"
```

### local.toml (per-machine identity, NOT tracked)

```toml
machine = "laptop"
identity = "~/.config/nit/age-key.txt"

[git]
strategy = "bare"  # "bare" (default) or "home"
```

## Design Principles

- **No auto-merge, ever.** "Clean merge" ≠ "correct merge." Source is the single source of truth for templates. Drift is saved, never incorporated automatically.
- **Every touchpoint shows drift.** You cannot interact with templates without being shown current drift and having your review recorded.
- **No interactive prompts.** No TTY input, ever. All output to stderr. Agents read errors, use their editor, re-run. Humans and AI agents use the same interface.
- **Gates that resolve themselves.** Blocking gates get bypassed under pressure. nit's commit gate shows information and writes an ack on first run, then proceeds on second run. No discipline required for safety.
- **Proportional friction.** Plain files: zero nit-specific overhead. Templates without drift: auto-proceed. Templates with drift: one extra run. The friction matches the risk.
- **Source wins is the safe default.** Wrongly discarding drift is recoverable (drift is saved). Wrongly merging junk is contamination (silent, hard to detect). Choose the recoverable failure mode.

## The Name

Swedish: **nit** = rivet (permanent metal fastener).
English: meticulous about details.

`git` tracks. `nit` fastens. Three letters each.

And yes — `nit pick` is the command that nitpicks your templates for drift.

## Installation

```bash
# From source (crates.io publication coming soon)
cargo install --git https://github.com/semikolon/nit.git
```

Or download pre-built binaries from [GitHub Releases](https://github.com/semikolon/nit/releases) for:
- `x86_64-unknown-linux-gnu` (Intel/AMD servers)
- `aarch64-unknown-linux-gnu` (Raspberry Pi, ARM servers)
- `aarch64-apple-darwin` (Apple Silicon Macs)

## Status

**Work in progress.** Core CLI skeleton, config loading, template discovery, smart `nit add`, and comprehensive tests (49) are implemented. Template rendering, encryption, triggers, and the ack system are coming next.

## Related

- **[hemma](https://github.com/semikolon/hemma)** — fleet provisioning orchestrator (Just-based). Shares `fleet.toml` with nit as a single source of truth for machine inventory. Handles SSH-based fleet operations, system overlays, and bootstrap.

## License

MIT
