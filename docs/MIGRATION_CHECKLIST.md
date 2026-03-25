# Migration Checklist: chezmoi → nit

Step-by-step guide for migrating a chezmoi-managed dotfiles repo to nit.

## Prerequisites

- [ ] nit installed (`cargo install --git https://github.com/semikolon/nit.git`)
- [ ] All concurrent CC/editor sessions closed (prevents mid-migration file edits)
- [ ] Working tree clean: `cd ~/dotfiles && git status` shows no uncommitted changes
- [ ] Remote up to date: `git push`
- [ ] Backup comfort: verify you can access the remote repo if something goes wrong

## Phase 1: Safety Net

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
# - File counts look right (plain, templates, secrets, scripts, symlinks)
# - Prefix resolution examples are correct
# - Template → target path mapping is correct
# - Trigger names and OS/role filters are correct
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

# Verify: git now sees $HOME as the work tree
git status | head -5
# Should show LOTS of untracked files (everything in $HOME)
```

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

**Checkpoint:** `git status` should now show a manageable list (dotfiles + dotfiles/).

## Phase 6: Restructure — Step 1 (Renames Only)

The key insight: **plain files don't physically move.** `~/.zshrc` already exists
(chezmoi deployed it). We just change what git tracks.

```bash
# Set up git alias for convenience (bare repo needs --git-dir every time)
alias nitgit='git --git-dir=$HOME/.local/share/nit/repo.git --work-tree=$HOME'

# Unstage all chezmoi source paths
nitgit rm -r --cached dotfiles/home/

# Stage plain files at their real $HOME paths
# (these already exist on disk — chezmoi deployed them)
nitgit add .zshrc .zshenv .zprofile .gitconfig .bashrc .bash_profile .profile
nitgit add .config/ .claude/ .hammerspoon/ .ssh/config
nitgit add .Brewfile .gemrc .vimrc .ackrc
nitgit add .local/bin/
nitgit add .git-templates/
nitgit add .claude-plugin/
nitgit add .codex/
# ... add all other plain files

# Move templates (physically — these get new paths)
mkdir -p dotfiles/templates
# For each .tmpl file: move from home/ to templates/, preserve directory structure
# Example: home/dot_zshenv.tmpl → dotfiles/templates/.zshenv.tmpl

# Move secrets
mkdir -p dotfiles/secrets
# For each .age file: move from home/private_dot_secrets/ to dotfiles/secrets/

# Move trigger scripts
mkdir -p dotfiles/scripts dotfiles/scripts/darwin dotfiles/scripts/linux
# For each script: move from home/.chezmoiscripts/ to dotfiles/scripts/

# Convert symlink_ files to real symlinks
# For each symlink_* file: read target, create ln -s

# Stage the moved files
nitgit add dotfiles/templates/ dotfiles/secrets/ dotfiles/scripts/

# COMMIT — pure renames, no content changes
# This maximizes git rename detection for history tracking
nitgit commit -m "nit: restructure from chezmoi source layout to nit layout

Moved 590+ plain files from home/dot_* to real \$HOME paths.
Moved 10 templates to dotfiles/templates/.
Moved 4 secrets to dotfiles/secrets/.
Moved 19 trigger scripts to dotfiles/scripts/.
Converted 6 symlinks to real ln -s.

Files tracked directly at their real paths — no more source/target split."
```

**Checkpoint:** `nitgit log --stat HEAD~1..HEAD` should show renames detected.

## Phase 7: Restructure — Step 2 (Template Conversion)

```bash
# Convert Go templates to Tera syntax
# The migration script's convert_go_to_tera() handles this
# Run it for each template in dotfiles/templates/

# For each .tmpl file in dotfiles/templates/:
#   1. Read current content (Go syntax from chezmoi)
#   2. Convert to Tera syntax
#   3. Write back

# Stage and commit — content changes only
nitgit add dotfiles/templates/
nitgit commit -m "nit: convert Go templates to Tera syntax

Mechanical conversion:
- {{ .chezmoi.hostname }} → {{ hostname }}
- {{ if eq .chezmoi.os \"darwin\" }} → {% if os == \"darwin\" %}
- {{ end }} → {% endif %}
- Docker format escapes → {% raw %} blocks
- Hash trigger comments removed (handled by triggers.toml)"
```

**Checkpoint:** the two-step commit ensures git tracks the rename (step 1) separately from
the content change (step 2), maximizing `git log --follow` history.

## Phase 8: Generate Config Files

```bash
# Generate fleet.toml (the migration script does this)
# Merges hemma/fleet.toml + .chezmoi.toml.tmpl data
nitgit add dotfiles/fleet.toml

# Generate triggers.toml (the migration script does this)
# Extracted from run_onchange_after_* naming + hash comments
nitgit add dotfiles/triggers.toml

# Stage .gitignore
nitgit add .gitignore

# Remove chezmoi metadata
nitgit rm --cached dotfiles/home/.chezmoiignore 2>/dev/null || true
nitgit rm --cached dotfiles/home/.chezmoi.toml.tmpl 2>/dev/null || true
nitgit rm --cached dotfiles/.chezmoiroot 2>/dev/null || true

nitgit commit -m "nit: add fleet.toml, triggers.toml, .gitignore; remove chezmoi metadata"
```

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

# Copy age key from chezmoi location (or generate new)
if [ -f ~/.config/chezmoi/key.txt ]; then
    cp ~/.config/chezmoi/key.txt ~/.config/nit/age-key.txt
    chmod 600 ~/.config/nit/age-key.txt
fi
```

## Phase 10: Verify

```bash
# nit should now work
nit status
# Expected: template count, trigger count, git status

nit list
# Expected: all templates, triggers, secrets listed

nit pick
# Expected: all templates clean (no drift — just deployed)

# Test git fall-through
nit log --oneline -5
# Expected: recent commits including migration commits

# Test that plain git from $HOME fails (by design)
cd ~ && git status
# Expected: "not a git repository" (correct — agents use nit, not git)

# Test nit from anywhere
cd ~/Projects && nit status
# Expected: works (bare repo doesn't depend on cwd)
```

## Phase 11: Push

```bash
nit push -u origin nit
# Pushes the nit branch to remote

# Verify on GitHub: nit branch has the restructured layout
```

## Phase 12: Optional — Set nit as Default Branch

Once verified and confident:

```bash
# On GitHub: change default branch from master to nit
# Then locally:
nit push origin --delete master  # or keep as rollback
```

## Rollback

At any point before Phase 12:

```bash
# Restore chezmoi layout
mv ~/.local/share/nit/repo.git ~/dotfiles/.git
cd ~/dotfiles
git config core.bare false
git config --unset core.worktree
git checkout master
chezmoi init --apply
```

## Post-Migration Cleanup (separate session)

- [ ] Update CLAUDE.md (global + project) — replace chezmoi references with nit
- [ ] Remove chezmoi wrappers: `chezmoi-drift-guard`, `chezmoi-auto-apply`, `chezmoi-post-apply`, `chezmoi-git`, `chezmoi-triage`
- [ ] Remove chezmoi from machines: `brew uninstall chezmoi` / `apt remove chezmoi`
- [ ] Update hemma to use `nit update` (already configured as primary)
- [ ] Set up publish trigger for public dotfiles repo (if applicable)
- [ ] Fleet migration: install nit on remote machines, run `nit bootstrap`
- [ ] Remove `chezmoi-final` tag when fully confident (optional — costs nothing to keep)
