# nit

AI-era dotfiles manager — git's rivet.

**nit** tracks dotfiles directly in `$HOME` via bare git. Edit your configs in place, commit with `nit commit`, push with `nit push`. For the handful of files that need per-machine differences, nit renders [Tera](https://keats.github.io/tera/) templates. For secrets, [age](https://age-encryption.org/) encryption with tiered access. For provisioning automation, hash-based triggers that fire only when watched files change.

```
$HOME (bare git work tree — plain files tracked directly)
  │
  ├── ~/.zshrc, ~/.config/*, ...  ← edit in place, plain git
  │
  └── ~/dotfiles/                 ← project hub (subdirectory of the same repo)
        ├── templates/            ← .tmpl source files
        ├── secrets/              ← .age encrypted files (tiered)
        ├── scripts/              ← trigger scripts
        ├── fleet.toml            ← machine inventory + config
        └── triggers.toml         ← hash-based trigger definitions
```

## Why nit?

### The one-machine story

You have a laptop. Your `.zshrc`, `.gitconfig`, and editor configs are exactly how you like them. You want them in git so they're safe and versioned. The simplest approach: track them with a bare git repo in `$HOME`. Edit a file, commit, push. Done.

nit does exactly this. `nit add ~/.zshrc && nit commit -m "update" && nit push` — it's just git with the right `--git-dir` flags so your home directory doesn't need a `~/.git` folder.

### The multi-machine story

Then you get a server. And maybe a Raspberry Pi. And a work laptop. Now you need:

- **The same base config everywhere** — your shell setup, git config, editor prefs
- **Per-machine differences** — your server needs different PATH entries, your Pi doesn't need your dev tools, your work laptop has different API keys
- **Secrets that don't leak** — API keys encrypted in git, but only your server can decrypt the production ones
- **Automated setup** — when your Brewfile changes, packages install automatically; when a service config changes, the service restarts

This is where most people reach for [chezmoi](https://www.chezmoi.io/) or [Nix Home Manager](https://nix-community.github.io/home-manager/). These are powerful tools, but they put *every* file through a transformation pipeline — even the 90%+ of files that are identical across all your machines.

nit takes a different approach: **plain git for plain files, nit for everything else.**

Most of your dotfiles are the same everywhere. They don't need templating, encryption, or any processing. nit tracks them directly in `$HOME` with bare git — no source/target split, no rename prefixes, no apply step. You edit the actual file.

For the ~5-10% that DO need per-machine treatment:

- **Templates** ([Tera/Jinja2](https://keats.github.io/tera/)): your `.zshenv` needs different exports on macOS vs Linux? Write a template with `{% if os == "darwin" %}`. nit renders it on each machine.
- **Secrets** ([age](https://age-encryption.org/)): API keys encrypted in git, with tiered access — your server decrypts production keys, your laptop only decrypts dev keys, your Pi gets neither.
- **Triggers**: your Brewfile changed? nit runs the package install script. A LaunchAgent plist changed? nit reloads it. Declarative, hash-based — only runs when files actually change.

### How nit compares

| | Plain files | Templates | Secrets | Triggers | Multi-machine |
|---|---|---|---|---|---|
| **Bare git** (manual) | Edit in place | No | No | No | Manual |
| **yadm** | Edit in place | Alt-files or Jinja2 | GPG | No | Manual |
| **chezmoi** | Source/target for all | Go templates | age | Hash-based scripts | chezmoi apply |
| **nit** | Edit in place | Tera (only where needed) | age (tiered) | Hash-based scripts | nit update |

nit gives you chezmoi's power (templates, encryption, triggers) without requiring every file to go through the pipeline. Plain files stay plain.

### Designed for concurrent AI workflows

nit is also built for environments where multiple AI agents or terminal sessions edit dotfiles simultaneously.

The core safety principle: **no template should be deployed without the person or agent seeing what's about to happen.** Every `nit commit` re-renders templates, deploys them to their target paths, and may trigger service restarts or app reloads that read from those files. But the target on disk might have been edited since the last deploy — a hotfix, an automated script that updated a config, or someone who didn't realize the file was managed by nit. Those edits need to be seen before nit overwrites them — otherwise you might lose a fix, or restart a service with config that silently dropped someone's changes.

nit enforces this structurally:

- **Drift shown at every touchpoint**: `nit add`, `nit apply`, `nit pick`, `nit commit` — all four show any difference between template source and target on disk. You can't interact with templates without seeing the current state.
- **Ack-gated commits**: `nit commit` refuses to proceed for template changes until you've reviewed the drift. No flag to bypass this — the tool enforces review, not your discipline.
- **Per-session isolation**: each terminal session tracks its own review state. No lock file, no contention between concurrent sessions. Scales to any number of agents.
- **No auto-merge**: drift is saved for conscious review, never silently incorporated. You decide what's junk and what's valuable.
- **No interactive prompts**: all output to stderr. No TTY input, ever. Agents and humans use the same interface.
- **Smart `nit add`**: detects template targets and redirects to staging the source — agents don't need to know which files are templates.

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

Most dotfiles are plain files — identical on every machine, tracked directly by git. But some files need to be *slightly different* per machine. Your `.zshenv` might need different `PATH` entries on macOS vs Linux. A LaunchAgent plist needs your actual home directory path baked in.

For these files, nit uses **templates**. A template is a file with placeholders that nit fills in for each machine:

```
# templates/.zshenv.tmpl (the source — what you edit)
export PATH="{{ home_dir }}/.local/bin:$PATH"
{% if os == "darwin" %}
export HOMEBREW_PREFIX="/opt/homebrew"
{% endif %}
```

When nit renders this template, it produces the actual file your shell reads:

```
# ~/.zshenv (the target — what nit generates)
# Managed by nit — edit templates/.zshenv.tmpl instead
export PATH="/Users/you/.local/bin:$PATH"
export HOMEBREW_PREFIX="/opt/homebrew"
```

**Source** = the template you edit (`templates/.zshenv.tmpl`)
**Target** = the rendered file on disk (`~/.zshenv`)

Templates live in `~/dotfiles/templates/` and mirror the target directory structure:

```
templates/.zshenv.tmpl              → ~/.zshenv
templates/.config/foo/config.tmpl   → ~/.config/foo/config
templates/Library/LaunchAgents/x.plist.tmpl → ~/Library/LaunchAgents/x.plist
```

Rendered files include a warning comment so you know not to edit them directly. Template variables include `hostname`, `os`, `arch`, `role`, and custom data from `fleet.toml`.

### What is "drift"?

Drift is when someone edits a rendered target directly instead of editing the template source. This happens naturally — you SSH into a server, hotfix `~/.zshenv` to unblock something, and forget to update the template. Now the file on disk differs from what nit would render.

### Why source always wins

When nit deploys, the template source always wins — the rendered output overwrites whatever is on disk. But the overwritten content is never lost. nit saves it as "drift" that you can review later.

Why not auto-merge the target edits back into the source? Because drift is roughly 50/50 junk vs valuable in practice, and the failure modes are asymmetric:

- **Wrongly discarding drift** → recoverable. Drift is saved to `~/.local/share/nit/drift/`, recoverable via `nit pick` at any time.
- **Wrongly merging junk** → contamination. Silent, hard to detect, and triggers may fire with bad config.

The safer default is the one with the recoverable failure mode.

### Drift handling walkthrough

**Scenario:** You SSH into a server and hotfix `~/.zshenv` directly. Later, `nit update` pulls new changes.

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

What if you (or an AI agent) accidentally run `nit add ~/.zshenv` — the rendered file, not the template? nit detects this and does the right thing automatically:

```bash
nit add ~/.zshenv
# nit: ~/.zshenv is a template target → staging source templates/.zshenv.tmpl
# nit: drift check for ~/.zshenv (full review with `nit pick`)

# Bulk staging works too:
nit add .
# Stages all modified tracked files via git
# Additionally scans all templates for drift
```

You don't need to remember which files are templates and which are plain. nit routes everything to the right place.

## The Ack System: Why It Works This Way

Plain files don't need special review — git's normal merge conflict handling is enough. But templates have two copies of the truth: the source (what you edit) and the target (what's on disk). These can diverge silently. The ack (acknowledgment) system ensures you've seen any divergence before committing.

### The problem it solves

Imagine this scenario:
1. You hotfix `~/.zshenv` on your server (editing the rendered target directly)
2. Meanwhile, you also update `templates/.zshenv.tmpl` (the template source)
3. You run `nit commit` — nit renders the template and overwrites your hotfix

Without a safety gate, step 3 silently destroys your hotfix. You might not notice for days.

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

## Managing Multiple Machines

If you only have one machine, you can skip this section. nit works great as a simple dotfile tracker. But when you have two or more machines sharing the same dotfiles repo, these features help keep them in sync.

### `fleet.toml` — your machine inventory

`fleet.toml` describes your machines. nit reads it to figure out "who am I?" and decide which templates to render, which secrets to decrypt, and which triggers to run.

```toml
[machines.laptop]
ssh_host = "laptop"        # matches your ~/.ssh/config
role = ["dev"]

[machines.server]
ssh_host = "server"
role = ["dev", "server"]
critical = true            # safety flag (see below)

[machines.router]
ssh_host = "router"
role = ["router"]
```

**Roles** drive per-machine behavior. A trigger with `role = "dev"` only runs on dev machines. A secret tier with recipients for `"server"` only decrypts on servers. A template exclusion with `unless_role = "dev"` skips rendering on non-dev machines.

Each machine also has a `local.toml` (not tracked in git) that says "I am `laptop`" — nit looks up the rest from fleet.toml.

If you also use [hemma](https://github.com/semikolon/hemma) for fleet orchestration, it reads the same fleet.toml — one file, both tools.

### `nit update` — syncing remote machines

On your remote machines (servers, Pis, etc.), `nit update` pulls the latest changes and applies them:

```bash
nit update         # git pull + render + decrypt + triggers
nit update --safe  # same, but skip service-restarting triggers (for production)
```

Key safety behavior: **drifted template targets are SKIPPED, not overwritten**. If someone SSH'd into a server and hotfixed a config, `nit update` preserves that fix and reports it. The reasoning: clobbering a production hotfix could cause an outage. The drift is saved for later review with `nit pick`.

You can run `nit update` manually via SSH, schedule it via cron, or let a fleet orchestrator like [hemma](https://github.com/semikolon/hemma) trigger it across all your machines in parallel.

### `critical = true` — protecting infrastructure

Marking a machine as `critical = true` signals to fleet orchestrators that it needs special care. Your router is also your DNS server — a bad config push takes down the network for every machine. hemma excludes critical machines from bulk operations and requires explicit confirmation.

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

**Work in progress.** Core implementation complete: config loading, template rendering, sync-base drift detection, ack system, age encryption, hash-based triggers, all CLI commands, bootstrap, permissions, GitHub Actions CI. 129 tests passing. Migration tooling ready. Currently in pre-migration testing.

## Companion: hemma

nit manages dotfiles on each machine locally. For orchestrating across multiple machines — SSH-based fleet sync, system config overlays for `/etc/`, bootstrapping new machines, managing configs that need root — there's **[hemma](https://github.com/semikolon/hemma)**.

hemma and nit are independent tools that work well together:
- **nit alone**: manage dotfiles on one or more machines (pull manually or via cron)
- **hemma alone**: manage system configs and fleet operations (works with chezmoi too)
- **nit + hemma**: full-stack fleet management — dotfiles, system configs, secrets, triggers, bootstrap

They share `fleet.toml` as a single source of truth for your machine inventory.

## License

MIT
