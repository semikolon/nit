# Migration Checklist: chezmoi → nit

Step-by-step guide for migrating a chezmoi-managed dotfiles repo to nit.

## Context for Continuation Sessions

**What exists:**
- `~/Projects/nit/` — the nit CLI tool (Rust, 129 tests, all commands functional). GitHub: [semikolon/nit](https://github.com/semikolon/nit)
- `~/Projects/hemma/` — fleet orchestrator (standalone). GitHub: [semikolon/hemma](https://github.com/semikolon/hemma)
- `~/Projects/nit/scripts/migrate-from-chezmoi.sh` — migration automation (dry-run tested, --execute mode implemented)
- `~/dotfiles/.claude/specs/nit/` — authoritative design spec (design.md, requirements.md, tasks.md)

**Current dotfiles structure (chezmoi):**
- `~/dotfiles/home/` is the chezmoi source dir (`.chezmoiroot`)
- Files use chezmoi naming: `dot_zshrc` → `~/.zshrc`, `private_dot_secrets/` → `~/.secrets/`, `executable_` prefix for +x
- 590 plain files, 10 templates (.tmpl), 4 secrets (.age), 19 trigger scripts, 6 symlinks
- chezmoi drift guard/watcher/triage all DISABLED (guard is `exit 0`)

**Target dotfiles structure (nit):**
- Bare git repo at `~/.local/share/nit/repo.git`, work tree = `$HOME`
- Plain files tracked at real paths (`~/.zshrc`, not `home/dot_zshrc`)
- Templates at `~/dotfiles/templates/*.tmpl` (Tera syntax, converted from Go)
- Secrets at `~/dotfiles/secrets/*.age`
- Trigger scripts at `~/dotfiles/scripts/{darwin,linux}/`
- `fleet.toml` + `triggers.toml` at `~/dotfiles/`
- `~/dotfiles/home/` directory removed from git (dead weight after migration)
- No `~/dotfiles/.git` — the bare repo at `~/.local/share/nit/repo.git` tracks everything

**Critical design decisions:**
- **Two-step commits**: step 1 = pure renames (maximizes `git log --follow` history tracking via content similarity), step 2 = Go→Tera conversion (content changes). Git detects renames by content similarity — if rename and content change are in one commit, similarity drops and history breaks.
- **Plain files don't physically move**: `~/.zshrc` already exists (chezmoi deployed it). We change what git TRACKS, not where files are on disk.
- **Templates DO physically move**: from `home/dot_zshenv.tmpl` to `dotfiles/templates/.zshenv.tmpl`
- **`~/dotfiles/` stays as project hub** — a subdirectory of the bare repo's work tree. It no longer has `.git/` (that moved to the bare location). Contains: templates/, secrets/, scripts/, system/, hemma/, docs/, fleet.toml, etc.
- **`~/dotfiles/home/` becomes dead weight** after migration — its contents are now tracked at their real $HOME paths. Back it up, then delete.
- **fleet.toml tier recipients**: migration script generates ALL tiers with ALL recipients (safe default). User must narrow tier-servers, tier-mac, tier-edge to correct machine subsets post-migration.
- **nit is NOT usable during migration** — it requires `local.toml` and `fleet.toml` which are created in Phases 8-9. Use `nitgit` alias (raw git with `--git-dir`/`--work-tree`) for all git operations during migration.

**What the migration script handles vs what's manual:**
- Script (`--execute`): template Go→Tera conversion, fleet.toml generation, triggers.toml generation, .gitignore creation, file categorization
- Manual (this checklist): git bare repo conversion, staging files at new paths, committing, verifying, pushing

**After Phase 1 migration, Phase 2 is planned:**
- Extract public dotfiles repo at `semikolon/dotfiles` (fresh repo, mirror from private bare repo)
- CLAUDE.md publish script with section-allowlist (new sections default private, safety net)
- nit trigger keeps public repo synced
- Excludes: `docs/claude/`, `system/` configs with IPs/passwords, security research

## Execution Plan (Apr 18, 2026)

Running migration from MERIAN via `ssh macmini` (tmux for connection safety). Mac Mini is the target machine. All Mac Mini CC sessions closed before executing. MERIAN repos synced to Mac Mini HEAD as of Apr 18.

**Pre-migration session (Apr 14) completed:** Ghostty config tracked, spela LAN-only bind, secret tier reorg (6 keys→tier-all), SSH config fleet-wide, iptables hardened, hemma overlay synced, hs CLI path fix (7 files), repo cleanup (6 repos), knot→overlay, "Fresh provision" directive forged, Mac Mini wedge root cause confirmed (Mos CGEventTap + TCC), chezmoi drift resolved to zero, all docs congruent. Migration script fixed: preserves git history (`mv .git`), ignores CC runtime, whitelists Ghostty.

## Prerequisites

- [x] nit installed (`cargo install --path .` on Mac Mini, v0.1.0, Apr 14)
- [ ] All concurrent CC/editor sessions closed (prevents mid-migration file edits — `.claude/` files will change)
- [x] Working tree clean: `cd ~/dotfiles && git status` shows no uncommitted changes (verified Apr 18)
- [x] Remote up to date: `git push` (verified Apr 18)
- [x] Backup comfort: `chezmoi-final` tag pushed to remote (Apr 14)
- [x] MERIAN repos synced to Mac Mini HEAD (Apr 18)
- [ ] **Audit untracked dependencies**: Any dotfiles manager only deploys what it tracks. Scripts, LaunchAgents, cron jobs, and symlinks created outside the managed source tree are invisible to migration — they'll keep working on *this* machine but won't exist on a fresh provision. Before migrating, inventory `~/.local/bin/`, `~/Library/LaunchAgents/` (macOS), `~/.config/systemd/user/` (Linux), and any other locations where you've placed ad-hoc scripts or services. Add them to the dotfiles source or document them as external dependencies. *(Audit: `docs/untracked_scripts_audit_2026_03.md` in the dotfiles repo.)*
- [ ] **Post-chezmoi services to provision**: These were created after the original audit and must be included:
  - `project-registry` — Rust binary ([semikolon/project-registry](https://github.com/semikolon/project-registry)). Build: `cargo install --path .` → `~/.local/bin/project-registry`. LaunchAgent: `com.fredrikbranstrom.project-registry.plist` (30s `refresh-if-stale`). Shared SSoT for project metadata (Redis DB 1).
  - `project-launcher` — Go binary ([semikolon/project-launcher](https://github.com/semikolon/project-launcher)). Build: `go build -ldflags="-s -w" -o ~/.local/bin/project-launcher-tui .` + shell wrapper at `~/.local/bin/project-launcher`. Ghostty config: `command = ~/.local/bin/project-launcher`, `shell-integration = detect`.
  - Ghostty config change: `command` and `shell-integration` lines added (not chezmoi-managed, lives at `~/Library/Application Support/com.mitchellh.ghostty/config`).
  - `.zshrc` alias: `pp` → `~/.local/bin/project-launcher-tui`. Already in chezmoi source (`home/dot_zshrc`).

## Phase 1: Safety Net ✅ (Apr 14)

```bash
cd ~/dotfiles

# Tag current state — this bookmark never gets deleted
git tag chezmoi-final
git push --tags

# Verify the tag exists on remote
git ls-remote --tags origin | grep chezmoi-final
```

**Checkpoint:** if anything goes wrong at any point, you can restore chezmoi:
```bash
git checkout master              # back to chezmoi layout
chezmoi init --apply             # restore chezmoi state
```

## Phase 2: Dry Run

```bash
# Review what the migration will do — no files modified
~/Projects/nit/scripts/migrate-from-chezmoi.sh

# Verify:
# - File counts: 590 plain, 10 templates, 4 secrets, 19 scripts, 6 symlinks
# - Prefix resolution: dot_zshrc → .zshrc, private_dot_secrets → .secrets, etc.
# - Template paths: dot_zshenv.tmpl → templates/.zshenv.tmpl
# - If the repo uses sccache, `home/dot_cargo/config.toml` is present as a plain file and shell init files do NOT carry `RUSTC_WRAPPER=...`
# - Trigger names and OS/role filters look correct
# - No unexpected files in any category
```

## Phase 3: Create nit Branch

```bash
cd ~/dotfiles
git checkout -b nit
```

All restructuring happens on this branch. `master` stays chezmoi-compatible as rollback.

## Phase 4: Convert Repo to Bare

```bash
# Move .git to bare location
mkdir -p ~/.local/share/nit
mv ~/dotfiles/.git ~/.local/share/nit/repo.git

# Configure as bare repo with $HOME as work tree
cd ~/.local/share/nit/repo.git
git config core.bare true
git config core.worktree "$HOME"
git config core.excludesFile "$HOME/.gitignore"
git config status.showUntrackedFiles no

# Verify: git now sees $HOME as the work tree
git --git-dir=$HOME/.local/share/nit/repo.git --work-tree=$HOME status | head -5
# Should show tracked files at dotfiles/ paths (old chezmoi layout)
```

**Note:** `~/dotfiles/` no longer has `.git/`. It's now just a regular directory — a subdirectory of the bare repo's work tree ($HOME). All git commands need `--git-dir` and `--work-tree` flags (or use the `nitgit` alias below).

## Phase 5: Create ~/.gitignore

```bash
cat > ~/.gitignore << 'EOF'
# nit blacklist strategy: ignore everything non-dot, whitelist project hub
# New dotfiles (.foorc) show up as untracked — add them with nit add

# Ignore all top-level non-dot items (Documents, Projects, Downloads, ...)
/*

# Whitelist the dotfiles project hub
!dotfiles/

# Ignore large/generated dotdirs
.cache/
.cargo/
.rustup/
.npm/
.local/share/nit/
.Trash/

# Ignore application state
.DS_Store
.CFUserTextEncoding
.lesshst
.viminfo
.zsh_history
.bash_history
.python_history
EOF
```

## Phase 6: Restructure — Step 1 (Renames Only)

The key insight: **plain files don't physically move.** `~/.zshrc` already exists
(chezmoi deployed it). We just change what git tracks. Templates, secrets, and
scripts DO physically move to new locations within `~/dotfiles/`.

```bash
# Set up git alias (bare repo needs --git-dir every time)
alias nitgit='git --git-dir=$HOME/.local/share/nit/repo.git --work-tree=$HOME'

# ── Unstage all chezmoi source paths ──
nitgit rm -r --cached dotfiles/home/

# ── Stage plain files at their real $HOME paths ──
# These already exist on disk (chezmoi deployed them).
# The content is IDENTICAL to the chezmoi source, so git detects renames.
#
# Use the migration script's dry-run output to get the full list.
# Key directories to stage:
nitgit add .zshrc .zshenv .zprofile .gitconfig .bashrc .bash_profile .profile
nitgit add .config/
nitgit add .claude/
nitgit add .claude-plugin/
nitgit add .codex/
nitgit add .hammerspoon/
nitgit add .ssh/config
nitgit add .git-templates/
nitgit add .local/bin/
nitgit add .Brewfile .gemrc .vimrc .ackrc
nitgit add .commit-template.txt
nitgit add .graphiti/
nitgit add .cargo/config.toml 2>/dev/null || true
# Check: nitgit status to see if any plain files were missed

# ── Move templates (physically) ──
mkdir -p ~/dotfiles/templates/Library/LaunchAgents
mkdir -p ~/dotfiles/templates/.graphiti

# Move each .tmpl file, stripping chezmoi prefixes from path:
# home/dot_zshenv.tmpl → dotfiles/templates/.zshenv.tmpl
# home/dot_zprofile.tmpl → dotfiles/templates/.zprofile.tmpl
# home/private_dot_graphiti/redis.conf.tmpl → dotfiles/templates/.graphiti/redis.conf.tmpl
# home/private_Library/LaunchAgents/*.tmpl → dotfiles/templates/Library/LaunchAgents/*.tmpl
# If you use sccache, keep that activation as a plain file:
#   home/dot_cargo/config.toml → tracked plain file ~/.cargo/config.toml
# not as `RUSTC_WRAPPER=...` inside .zshenv/.zshrc templates.
# (use migration script's convert_go_to_tera for step 2, but move WITHOUT conversion first)
cp ~/dotfiles/home/dot_zshenv.tmpl ~/dotfiles/templates/.zshenv.tmpl
cp ~/dotfiles/home/dot_zprofile.tmpl ~/dotfiles/templates/.zprofile.tmpl
cp ~/dotfiles/home/private_dot_graphiti/redis.conf.tmpl ~/dotfiles/templates/.graphiti/redis.conf.tmpl
for f in ~/dotfiles/home/private_Library/LaunchAgents/*.tmpl; do
    cp "$f" ~/dotfiles/templates/Library/LaunchAgents/"$(basename "$f")"
done

# ── Move secrets (physically) ──
mkdir -p ~/dotfiles/secrets
cp ~/dotfiles/home/private_dot_secrets/encrypted_private_tier-all.env.age ~/dotfiles/secrets/tier-all.env.age
cp ~/dotfiles/home/private_dot_secrets/encrypted_private_tier-servers.env.age ~/dotfiles/secrets/tier-servers.env.age
cp ~/dotfiles/home/private_dot_secrets/encrypted_private_tier-mac.env.age ~/dotfiles/secrets/tier-mac.env.age
cp ~/dotfiles/home/private_dot_secrets/encrypted_private_tier-edge.env.age ~/dotfiles/secrets/tier-edge.env.age

# ── Move trigger scripts (physically) ──
mkdir -p ~/dotfiles/scripts/darwin ~/dotfiles/scripts/linux
# Copy from home/.chezmoiscripts/ to scripts/, stripping run_onchange_after_ prefix
# Use the migration script for the full mapping (or do it manually)

# ── Convert symlinks ──
# For each symlink_* file, create a real symlink:
# home/dot_claude/symlink_agents.md → target is "CLAUDE.md"
# → create real symlink: ln -sf CLAUDE.md ~/.claude/agents.md
# (check each one: cat ~/dotfiles/home/dot_claude/symlink_agents.md to see target)

# ── Stage moved files ──
nitgit add dotfiles/templates/ dotfiles/secrets/ dotfiles/scripts/

# ── Remove chezmoi metadata from git ──
nitgit rm --cached dotfiles/home/.chezmoiignore 2>/dev/null || true
nitgit rm --cached dotfiles/home/.chezmoi.toml.tmpl 2>/dev/null || true
nitgit rm --cached dotfiles/home/.chezmoi-commit-message.tmpl 2>/dev/null || true
nitgit rm --cached dotfiles/.chezmoiroot 2>/dev/null || true

# ── COMMIT — pure renames, no content changes ──
# This maximizes git's rename detection (content similarity = 100% for plain files)
nitgit commit -m "nit: restructure from chezmoi source layout to nit layout

Moved 590+ plain files from home/dot_* to real \$HOME paths.
Moved 10 templates to dotfiles/templates/.
Moved 4 secrets to dotfiles/secrets/.
Moved 19 trigger scripts to dotfiles/scripts/.
Converted 6 symlinks to real ln -s.
Removed chezmoi metadata (.chezmoiroot, .chezmoiignore, .chezmoi.toml.tmpl).

Files tracked directly at their real paths — no more source/target split."
```

**Checkpoint:** `nitgit log --stat HEAD~1..HEAD` should show renames detected (R100 = perfect rename).

## Phase 7: Restructure — Step 2 (Template Conversion)

Separate commit so git sees the rename (Phase 6) and content change (Phase 7) independently.

```bash
# Convert Go templates to Tera syntax using the migration script's converter.
# The convert_go_to_tera() function is a Python script embedded in the migration script.
# Extract and run it on each template:

for tmpl in ~/dotfiles/templates/*.tmpl ~/dotfiles/templates/**/*.tmpl; do
    [ -f "$tmpl" ] || continue
    # The migration script has convert_go_to_tera — source it or run inline
    # Key conversions:
    #   {{ .chezmoi.hostname }} → {{ hostname }}
    #   {{ .chezmoi.os }} → {{ os }}
    #   {{ .chezmoi.homeDir }} → {{ home_dir }}
    #   {{ if eq .chezmoi.os "darwin" }} → {% if os == "darwin" %}
    #   {{ end }} → {% endif %}
    #   {{ if not .is_dev }} → {% if not is_dev %}
    #   Docker format escapes (backtick) → {% raw %} blocks
    #   Hash trigger comments → removed (handled by triggers.toml)
done

# Stage and commit — content changes only
nitgit add dotfiles/templates/
nitgit commit -m "nit: convert Go templates to Tera syntax

Mechanical conversion — same logic, different syntax.
Docker format escapes use {% raw %} blocks.
Hash trigger comments removed (handled by triggers.toml)."
```

**Verification:** compare rendered output on a template:
```bash
# Before: what chezmoi rendered
cat ~/.zshenv

# After: what nit would render (once nit is bootstrapped in Phase 9)
nit apply dotfiles/templates/.zshenv.tmpl
diff ~/.zshenv <(nit apply --dry-run .zshenv)  # conceptual — exact syntax TBD
```

## Phase 8: Generate Config Files

```bash
# Run the migration script in execute mode to generate fleet.toml + triggers.toml
# (or generate manually based on the dry-run output)
~/Projects/nit/scripts/migrate-from-chezmoi.sh --execute
# This generates: fleet.toml, triggers.toml, .gitignore (if not already created)

# Stage new config files
nitgit add dotfiles/fleet.toml
nitgit add dotfiles/triggers.toml
nitgit add .gitignore

nitgit commit -m "nit: add fleet.toml, triggers.toml, .gitignore"
```

**Important:** The generated `fleet.toml` has ALL age recipients in EVERY tier.
You MUST narrow the tier recipients to match the actual access model:
- tier-all: all machine keys
- tier-servers: only Mac Mini + Darwin keys
- tier-mac: only Mac Mini + MBP keys
- tier-edge: only Mac Mini + Shannon keys

## Phase 9: Generate Machine Identity

```bash
# Create local.toml (per-machine, NOT tracked)
mkdir -p ~/.config/nit
cat > ~/.config/nit/local.toml << EOF
machine = "$(hostname -s | tr '[:upper:]' '[:lower:]' | tr ' ' '-')"
identity = "~/.config/nit/age-key.txt"

[git]
strategy = "bare"
EOF

# Copy age key from chezmoi location
if [ -f ~/.config/chezmoi/key.txt ]; then
    cp ~/.config/chezmoi/key.txt ~/.config/nit/age-key.txt
    chmod 600 ~/.config/nit/age-key.txt
fi

# Verify local.toml machine name matches a fleet.toml entry
cat ~/.config/nit/local.toml
grep "$(cat ~/.config/nit/local.toml | grep machine | cut -d'"' -f2)" dotfiles/fleet.toml
```

## Phase 10: Verify

```bash
# nit should now work (reads local.toml + fleet.toml)
nit status
# Expected: N templates, N triggers | git: X staged, Y modified, Z untracked

nit list
# Expected: all templates, triggers, secrets listed with status

nit pick
# Expected: all templates clean (no drift — just deployed by chezmoi)

# Test git fall-through
nit log --oneline -5
# Expected: recent commits including migration commits

# Test that plain git from $HOME fails (by design)
cd ~ && git status
# Expected: "not a git repository" (correct — agents use nit, not git)

# Test nit from any directory
cd ~/Projects && nit status
# Expected: works (bare repo doesn't depend on cwd)

# Verify a template renders correctly
nit apply
# Should render all templates without errors
```

## Phase 11: Clean Up Dead Weight

```bash
# Back up the old chezmoi home/ directory (just in case)
tar czf ~/dotfiles-home-backup-$(date +%Y%m%d).tar.gz -C ~/dotfiles home/

# Remove from disk (already unstaged from git in Phase 6)
rm -rf ~/dotfiles/home/

# Also clean up chezmoi-specific files that are now in home/ backup:
# .chezmoiroot, .chezmoi-commit-message.tmpl, etc.
# These were removed from git in Phase 6 but may still be on disk
rm -f ~/dotfiles/.chezmoiroot
```

## Phase 12: Push

```bash
nit push -u origin nit
# Pushes the nit branch to remote

# Verify on GitHub: nit branch has the restructured layout
# - No home/ directory
# - dotfiles/templates/ with .tmpl files (Tera syntax)
# - dotfiles/secrets/ with .age files
# - dotfiles/scripts/ with trigger scripts
# - dotfiles/fleet.toml + triggers.toml
# - Plain files tracked at root (.zshrc, .config/, .claude/, etc.)
```

## Phase 13: Optional — Set nit as Default Branch

Once verified and confident (maybe after a day or two):

```bash
# On GitHub: change default branch from master to nit (or main)
# Then locally:
nit push origin --delete master  # or keep as rollback forever
```

## Rollback

At any point before Phase 13:

```bash
# Restore the git repo to ~/dotfiles/
mv ~/.local/share/nit/repo.git ~/dotfiles/.git
cd ~/dotfiles
git config core.bare false
git config --unset core.worktree
git config --unset core.excludesFile
git config --unset status.showUntrackedFiles
git checkout master
chezmoi init --apply
```

## Post-Migration Cleanup (separate session)

- [ ] Update CLAUDE.md (global + project) — replace chezmoi references with nit
- [ ] If using sccache, verify `~/.cargo/config.toml` carries `build.rustc-wrapper` and no shell init file reintroduces `RUSTC_WRAPPER=...`
- [ ] Remove chezmoi wrappers: `chezmoi-drift-guard`, `chezmoi-auto-apply`, `chezmoi-post-apply`, `chezmoi-git`, `chezmoi-triage`, `chezmoi-re-encrypt`, `chezmoi-update-if-idle`
- [ ] Remove chezmoi from machines: `brew uninstall chezmoi` / `apt remove chezmoi`
- [ ] Narrow fleet.toml tier recipients — key mapping at `docs/age_key_mapping.md` (4/5 confirmed, Shannon repurposed — drop both unconfirmed keys)
- [ ] Update hemma to use `nit update` (already configured as primary, verify)
- [ ] Set up publish trigger for public dotfiles repo (`semikolon/dotfiles`)
- [ ] Fleet migration: install nit on remote machines, run `nit bootstrap`
- [ ] Remove `chezmoi-final` tag when fully confident (optional — costs nothing to keep)
- [ ] Evaluate CI runners for nit release builds — Blacksmith (Linux, 3K free min/mo) + GetMac (macOS M4) for cross-compiling fleet binaries. Research: `dotfiles/docs/ci_runner_alternatives_2026_03.md`
