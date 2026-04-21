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

## Execution Update (Apr 21, 2026 — about to run)

**Plan change**: User returned from travel without running the migration on MERIAN. Running directly **from Mac Mini** instead. The current prep CC session (`d10658c7-4d43-4ba7-965c-1d5a75128ec6`) will be quit before `--execute`, then resumed via `claude --resume d10658c7-4d43-4ba7-965c-1d5a75128ec6` to do the manual restructure with full context. **If resume fails for any reason**, this section + the immediate-post-script steps below are sufficient to continue from a fresh session.

### State snapshot at script-run time

- **HEAD**: `b5b0217` on `master` — `feat(tier0): pyleak integration + Q2/Q3 minimal prompt tweaks + docs congruence`
- **Tags pushed to origin**: `chezmoi-final` (`0a188ec`, Apr 14, now 115 commits stale) + **`pre-nit`** (`b5b0217`, Apr 21, fresh rollback bookmark)
- **chezmoi LaunchAgents**: both `.plist.disabled` and absent from `launchctl list` ✓ — no background writers
- **chezmoi status**: only 2 run-script triggers pending (`25-build-rust-hooks.sh`, `darwin/30-reload-launchagents.sh`) — NOT target drift, safe to ignore
- **Active CC sessions**: just the prep session (PID 49807, will be quit before `--execute`)
- **`~/.local/share/nit/`**, **`~/.config/nit/`**: don't exist yet (clean slate for Phases 7+8) ✓
- **Age key**: present at `~/.config/chezmoi/key.txt` (manual copy needed post-script — script does NOT copy it)

**Dry run actuals (vs Apr 14 estimates earlier in this doc)**:

| | Doc says | Actual (Apr 21 dry run) |
|---|---|---|
| Plain files | 590 | **675** |
| Templates | 10 | **11** |
| Secrets | 4 | 4 |
| Trigger scripts | 19 | **18** |
| Symlinks | 6 | 6 |

Deltas come from Apr 14–20 work (Tier 0 dedup, pyleak integration, Ruby identity-bleed fix, daemon reliability fixes, SuperWhisper settings JSON, etc.). All in chezmoi source — migrates cleanly.

### Discoveries that supersede instructions further down this doc

1. **`nitgit` alias is NOT needed.** nit's CLI uses `clap::external_subcommand` (`nit/src/main.rs:111`) — any unrecognized subcommand falls through to git with the bare strategy hardcoded (`nit/src/git.rs:92`). After Phase 7 of the script creates the bare repo, just use `nit rm -r --cached dotfiles/home/`, `nit add .zshrc`, `nit commit -m "..."`, `nit log`, `nit push -u origin nit` — all work directly. **Skip every `alias nitgit=...` instruction in Phases 6–12 below; substitute `nit` for `nitgit` throughout.**

2. **Hostname won't auto-match `macmini`.** Mac Mini's hostname is `Fredriks-Mac-Mini`; the script's case-insensitive substring matcher fails because of the hyphens (`"macmini" not in "fredriks-mac-mini"`). Script falls back to literal hostname and warns. **Manual fix required**: edit `~/.config/nit/local.toml` and change `machine = "Fredriks-Mac-Mini"` → `machine = "macmini"`.

3. **Script does Go→Tera conversion in a SINGLE pass** (during Phase 3 file copy), not the two-step "rename then convert" the doc's Phases 6–7 below describe. Plain files (the bulk) are still R100 renames since they're never modified; only templates lose perfect rename detection. Acceptable cost — Go→Tera diffs are mostly token swaps (`.chezmoi.X` → `X`), so similarity stays well above git's 50% threshold.

4. **Tier recipients are wide-open by default**. Generated `fleet.toml` puts ALL age recipients in EVERY tier. Not exploitable today (we trust our own machines) but defeats the tier model. Narrowing per `docs/age_key_mapping.md` is in the post-migration cleanup list — do it before adding any new fleet machine.

### Immediate post-script steps (in this exact order)

If resume succeeds, the prep session walks through these. If resume fails, do them by hand:

1. **Read script stderr / exit code.** If non-zero, STOP and consult before doing anything else.
2. **Sanity-check generated files**:
   ```bash
   cat ~/dotfiles/fleet.toml
   cat ~/dotfiles/triggers.toml
   cat ~/.config/nit/local.toml
   cat ~/.gitignore
   ```
3. **Verify bare repo intact**:
   ```bash
   git --git-dir=$HOME/.local/share/nit/repo.git rev-parse HEAD
   # Expected: b5b02178c41095f112320f9e90fd99fef32b555c (or wherever HEAD was when script ran)
   ```
4. **Copy age key** (script does NOT do this):
   ```bash
   cp ~/.config/chezmoi/key.txt ~/.config/nit/age-key.txt
   chmod 600 ~/.config/nit/age-key.txt
   ```
5. **Fix machine name in local.toml**:
   ```bash
   sed -i.bak 's/machine = "Fredriks-Mac-Mini"/machine = "macmini"/' ~/.config/nit/local.toml
   cat ~/.config/nit/local.toml  # verify
   ```
6. **First nit health check**:
   ```bash
   nit status
   # Expected: config loads cleanly, bare repo found, no errors
   ```
7. **Verify hook symlinks survived intact** (script rm+ln'd them — should still point to `narrate-client`):
   ```bash
   ls -la ~/.claude/hooks/{stop,session_start,session_end,notification}.sh ~/.claude/agents.md ~/.codex/AGENTS.md
   ```
8. **Then proceed with Phase 6 (manual restructure) below** — but use plain `nit` everywhere the doc says `nitgit`.

### Rollback if anything goes wrong before push

```bash
# Restore the git repo to ~/dotfiles/
mv ~/.local/share/nit/repo.git ~/dotfiles/.git
cd ~/dotfiles
git config core.bare false
git config --unset core.worktree
git config --unset core.excludesFile
git config --unset status.showUntrackedFiles
git checkout master       # back to b5b0217 (pre-nit tag)
chezmoi init --apply      # restore chezmoi state
```

The `pre-nit` tag is the current-state bookmark; `chezmoi-final` is the older Apr 14 fallback if `pre-nit` is somehow corrupted.

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

> **🤖 AUTOMATED (Apr 21, 2026):** This phase is now scripted. Run:
> ```bash
> ~/Projects/nit/scripts/migrate-from-chezmoi-restage.sh           # dry-run
> ~/Projects/nit/scripts/migrate-from-chezmoi-restage.sh --execute # do it
> ```
> The script implements all the manual steps below as one big commit
> (intra-commit rename detection). Reading the manual steps below is
> still useful to understand WHAT the script does and why. The pre-flight
> audit's gitignore additions are baked into Phase B of the script.
>
> **NOTE (Apr 21, 2026):** Use plain `nit` instead of `nitgit` throughout this phase — see the "Execution Update (Apr 21, 2026)" section above for why. Also: the script's `--execute` mode already did the Go→Tera template conversion in a single pass during Phase 3, so the "two-step" rationale below applies cleanly to plain files (R100 renames) but templates will show as similarity-based renames, not R100. That's expected and fine.

> **🔴 CRITICAL RISKS (Apr 21, 2026)** — discovered during Mac Mini migration prep. Read before staging anything.
>
> 1. **Three file classes, not two.** Beyond (a) chezmoi source under `home/dot_*` and (b) moved templates/secrets/scripts, there's (c) **the dotfiles repo's OWN state files** — `~/dotfiles/.claude/{plans,specs,session_reports}/`, `~/dotfiles/docs/`, `~/dotfiles/hemma/`, `~/dotfiles/system/`, `~/dotfiles/scripts/` (pre-existing utility scripts), `~/dotfiles/tests/`, etc. These were tracked at root-level paths (`.claude/...`, `docs/...`) relative to the OLD work tree (`~/dotfiles/`). After the bare-repo move with `core.worktree=$HOME`, these tracked paths now resolve to `~/.claude/...` instead of `~/dotfiles/.claude/...` (which is where the files actually live). Phase 6 must **untrack at the bare path AND restage at `dotfiles/<X>` paths** for each top-level dir under `~/dotfiles/` that isn't `home/`. Pre-flight to enumerate the surface:
>    ```bash
>    nit ls-tree HEAD --name-only | awk -F/ '{print $1}' | sort -u
>    ```
>
> 2. **`/*` gitignore blocks non-dot tracked paths.** The script's `~/.gitignore` whitelists only `dotfiles/`, `bin/`, and specific Ghostty paths under `Library/`. But chezmoi tracked some non-dot paths (e.g. `home/private_Documents/superwhisper/settings/settings.json` → would restage at `~/Documents/superwhisper/...`) — **blocked by gitignore**. Pre-flight to find them:
>    ```bash
>    nit ls-tree HEAD --name-only | grep -v '^\.' | awk -F/ '{print $1}' | sort -u
>    ```
>    Whitelist (e.g. add `!Documents/superwhisper/` to `~/.gitignore`) or accept loss.
>
> 3. **Rename detection is intra-commit only.** If you `nit rm --cached old` in commit A and `nit add new` in commit B, **no rename is detected** → `git log --follow` breaks for that file. All untrack+restage pairs MUST land in the SAME commit per file class. One big commit is safer than splitting.
>
> 4. **`diff.renameLimit` default is 1000 — the bare repo has 1640+ tracked files.** Bump it BEFORE the commit to avoid silent rename-detection skip:
>    ```bash
>    nit config diff.renameLimit 5000
>    nit config merge.renameLimit 5000
>    ```
>
> 5. **Symlink mode change**: 6 hook symlinks (`stop.sh`, `notification.sh`, `session_start.sh`, `session_end.sh`, `~/.codex/AGENTS.md`, `~/.claude/agents.md`) were tracked as regular files containing the target string (e.g. `narrate-client`). New state has them as real symlinks (mode 120000). Same blob content but different mode → rename detection may miss; history reachable via blob hash but `git log --follow` may break for these 6.
>
> **Recommended single-commit sequence** (within ONE commit, in this order):
> 1. `nit rm -r --cached home/` (untrack 590+ chezmoi source files)
> 2. `nit rm --cached .chezmoiroot .chezmoiignore .chezmoi.toml.tmpl .chezmoi-commit-message.tmpl 2>/dev/null` (untrack chezmoi metadata)
> 3. For each non-`home/` top-level tracked dir at `~/dotfiles/<X>/` (use the pre-flight enumeration to find them — likely `.claude`, `docs`, `hemma`, `scripts`, `system`, `tests`): `nit rm -r --cached <X>` then `nit add dotfiles/<X>`
> 4. Stage plain files at $HOME paths: `nit add .zshrc .config .claude .codex .hammerspoon .ssh/config .git-templates .local/bin .Brewfile .gemrc .vimrc .ackrc .commit-template.txt .graphiti .bashrc .bash_profile .profile` (adjust list using dry-run output)
> 5. Stage moved files + new configs: `nit add dotfiles/templates dotfiles/secrets dotfiles/scripts dotfiles/fleet.toml dotfiles/triggers.toml`
> 6. Single commit. Then verify rename detection: `nit log --stat HEAD~1..HEAD | grep -c "^ rename"` — should show many R lines.

> **📋 GITIGNORE AUDIT (Apr 21, 2026 — applied)** — pre-flight done; bulk staging is now safe.
>
> **Two critical bugs in script-generated `~/.gitignore`** (both fixed in script `baa5ff1+`, but if running an older migration script, apply manually):
>
> 1. **`/*` blanket-ignores dot-prefixed dirs.** The script generated `/*` to "ignore all top-level non-dot items" — but `/*` matches BOTH dot and non-dot top-level entries. Result: `.claude`, `.config`, `.codex`, `.hammerspoon`, `.ssh`, `.local`, `.git-templates`, `.graphiti`, even individual dotfiles (`.zshrc`, `.zshenv`) are ALL silently blocked from being tracked. `nit add .claude` would refuse with "ignored by gitignore". **Fix**: replace `/*` with `/[!.]*` in `~/.gitignore`. Now only non-dot entries (Documents, Downloads, Movies, etc.) are blanket-ignored.
>
> 2. **Gitignore does NOT support inline comments.** Lines like `.config/sccache/   # compile cache` are parsed as a literal pattern containing whitespace + `#`, which matches nothing. Comments must be on their own lines. Audit any added pattern that has `   # ` and split.
>
> **Comprehensive runtime/build/secret patterns added** (~70 patterns) — without these, `nit add .claude/` would pull in 600+MB of build artifacts. Categories applied:
>
> - **Build artifacts**: `**/__pycache__/`, `*.pyc`, `*.pyo`, `**/.venv/`, `**/.virtualenv/`, `**/target/`
> - **Backup/temp/log**: `*.bak`, `*.bak[0-9]*`, `*.backup`, `*.old`, `*.orig`, `*~`, `*.log`, `**/lock.mdb`
> - **Built CC hook binaries** (per-machine, regenerated by `25-build-rust-hooks.sh`): `.claude/hooks/{narrate-client,context-nudge,skill-preeval,tts-cli,check_mic_active}`, `.local/bin/{notification-reader,tts-cli}`, `.claude/hooks/mic-process-monitor/mic-process-monitor`
> - **CC runtime state**: `.claude/{plugins,cache,paste-cache,tasks,telemetry,backups,teams,contextual-intelligence,audio_cache,audio_test_results,away_queue.json,tier2_trigger.json,stats-cache.json,suggestions_log.jsonl,significance_state.json,settings.local.json.backup,mcp-needs-auth-cache.json,.last_user_prompt_timestamp,.claude.backup-*}`, `.claude/hooks/{cache,audio_cache,away_queue.json,archive,.archived-*,.backup-*/,.backups,mcp_secrets.env}`
> - **Codex runtime**: `.codex/{auth.json,cache,history.jsonl,installation_id,sessions,session_index.jsonl,shell_snapshots,tmp,vendor_imports,version.json,state_*.sqlite*,.tmp,log,logs_*.sqlite*,models_cache.json,.codex-global-state.json,.personality_migration,plugins/cache,.claude}`
> - **Sensitive secrets** (NEVER commit): `.config/{chezmoi/key.txt,nit/age-key.txt,nit/local.toml,keygate/approver.key}`, `.ssh/{id_*,known_hosts*,authorized_keys,agent.sock}`, `.claude/hooks/mcp_secrets.env`
> - **App runtime under .config/**: `.config/{sccache,playwright-mcp,system-sentinel/state*,mise/.cache,fabric/{patterns,contexts,extensions,sessions,.env,unique_patterns.txt},karabiner/automatic_backups,cagent,chezmoi,uv/uv-receipt.json}` + `!.config/fabric/patterns/triage_*/` whitelist for user's custom fabric patterns
> - **`.local` runtime dirs**: `.local/{share,state,lib,src}` (chezmoi only tracked `.local/bin/` scripts)
> - **External git-clone projects** (have own `.git/`, can't be sub-tracked): `.config/claude/` (claude-shim-pipeline)
>
> **Final candidate counts after audit** (vs chezmoi-tracked baseline):
>
> | Dir | Before audit | After audit | Note |
> |---|---|---|---|
> | `.claude` | 2904 | 360 | +30 vs chezmoi 330 — new TTS daemon files, speaker profiles |
> | `.codex` | 195 | 67 | +49 vs chezmoi 18 — mostly skills/.system/, all legit |
> | `.config` | 371 | 35 | +15 vs chezmoi 20 — new codex shims/wrappers |
> | `.hammerspoon` | 126 | 126 | +1 vs chezmoi 125 — clean |
> | `.local` | 461 | 58 | +38 vs chezmoi 20 — chezmoi-* wrappers + new helper scripts |
> | `.ssh` | 10 | 1 | 0 — just `config` left |
> | `.git-templates` | 2 | 2 | 0 — clean |
> | `.claude-plugin` | 4 | 3 | 0 — clean |
> | `.graphiti` | 0 | 0 | 0 — clean |
> | **Total** | **4073** | **654** | vs chezmoi 632; 22-net-extras are legitimate new content |
>
> **Verification command** (run before bulk-staging):
> ```bash
> for top in .claude .codex .config .hammerspoon .local .git-templates .graphiti .ssh .claude-plugin; do
>   count=$(cd / && git --git-dir=$HOME/.local/share/nit/repo.git --work-tree=$HOME add -An "$top" 2>&1 | grep -c "^add")
>   echo "$top: $count"
> done
> ```
> Compare your numbers against the table above. Investigate any dir with >2x the expected count before staging.
>
> **Embedded git repos**: 9 found during audit. All correctly handled by gitignore now: `.claude/plans/.git`, `.claude/plugins/{cache,marketplaces}/...` (4), `.codex/vendor_imports/skills/.git`, `.codex/.tmp/plugins/.git`, `.config/claude/.git`, `.local/src/project-launcher/.git`. Don't sub-track these — they have their own version control.

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
- [x] Fleet migration: install nit on remote machines, run `nit bootstrap`
  - [x] MERIAN migrated (Apr 21, 2026): `cargo install --git ... --root ~/.local`, `nit bootstrap git@github.com:semikolon/dotfiles.git`, 12 templates rendered, 5 triggers fired cleanly on first pass, additional 7 after script-hash auto-watch fix
  - [x] Darwin migrated (Apr 21, 2026): same procedure, sccache 0.14.0 installed, sluss build succeeded after cargo-config template deployment, all services preserved through bootstrap (kamal-proxy, brf-auto, FalkorDB, Redis, Temporal, ntfy)
  - [ ] Turing — deferred (unplugged weeks; when plugged in: reactivation per dotfiles/TODO.md § Phase 1.6)
  - [ ] Shannon — deferred (cold spare; when powered up: same procedure + add age pubkey to fleet.toml tier-edge recipients, then `nit rekey`)
  - Fleet rollout surfaced 4 additional nit bugs (all fixed + pushed): Tera leftovers in 15 trigger scripts, no-watch trigger script-hash auto-watch, bootstrap upstream tracking + merge mode, LaunchAgent PATH-based nit binary. Full lesson list: `~/dotfiles/docs/DOTFILES_STRATEGY.md` § "Fleet rollout lessons (Apr 21, 2026)".
  - chezmoi binary uninstalled fleet-wide (Mac Mini brew, MERIAN direct, Darwin direct). chezmoi-* wrapper scripts untracked via nit. `~/.local/share/chezmoi/` + `~/.config/chezmoi/` purged.
  - Nightly sync wired: macOS LaunchAgent template at `dotfiles/templates/Library/LaunchAgents/com.fredrikbranstrom.nit-update.plist.tmpl` + Linux cron via `dotfiles/scripts/linux/22-setup-nit-update.sh`. Both run `exec nit update` at 03:00 daily with PATH-based binary resolution.
  - hemma migrated to nit-aware paths (update/status/apply/apply-all/bootstrap recipes). `hemma status` reads per-machine `last-sync.json` via SSH.
  - **Sacred drift-safety principle shipped** (new AC-9.6 + AC-9.7): `nit update` aborts on work-tree drift + writes `last-sync.json`. Design detail: `dotfiles/.claude/specs/nit/design.md` § "Plain-file drift: abort-and-notify".
- [ ] Remove `chezmoi-final` tag when fully confident (optional — costs nothing to keep)
- [ ] Evaluate CI runners for nit release builds — Blacksmith (Linux, 3K free min/mo) + GetMac (macOS M4) for cross-compiling fleet binaries. Research: `dotfiles/docs/ci_runner_alternatives_2026_03.md`
- [ ] **Forward-only sync semantics for append-only runtime data** (Apr 20, 2026): chezmoi's source-wins-on-apply is the wrong model for files primarily *written* by runtime processes rather than *edited* by humans. Today this works only because `chezmoi-auto-apply` is disabled — a stray `chezmoi apply --force <target>` silently overwrites newer runtime state with the older source snapshot, losing content between the last `re-add` and now. Affected files include:
  - `~/.claude/decisions_graphiti_cache.jsonl` — the JSONL leg of the triple-store (`Graphiti + DECISIONS.md + JSONL`, per global CLAUDE.md § "Knowledge Management System")
  - `~/.claude/decisions_state.json` — derived recent-decisions cache
  - `~/.claude/logs/ruby_conversations.jsonl` (currently gitignored but conceptually same class)
  - Future: any tier-2 analyses, session retrospectives, or other runtime-accumulated logs that deserve permanence

  **Proposed systemic solution for nit** — extend the per-file classification system with a new class:
  ```toml
  # In triggers.toml / fleet.toml / a new sync.toml — whatever fits nit's model
  [[sync]]
  path = ".claude/decisions_graphiti_cache.jsonl"
  direction = "forward_only"  # target → source, never source → target (except bootstrap)
  ```
  Semantics:
  - **Bootstrap** (fresh machine, target doesn't exist): source → target, so the historical snapshot arrives with the dotfiles.
  - **Steady state** (target exists): `nit update` leaves the target untouched. Runtime appends freely.
  - **Flush**: a user-invoked (or watcher-driven) `nit sync` copies target → source, commits (optionally pushes). Conflicts can't happen because target is always ahead.
  - **Safety**: `nit apply --force` on a `forward_only` file is either a no-op or refuses — removes the "silent overwrite" failure mode entirely. This is the "auto-resolving gates over blocking gates" principle from global CLAUDE.md applied to sync: the gate resolves by *declining to destroy data*, not by prompting.

  **Fully-automated flavor** (optional v2): a fswatch-style watcher, debounced ~1 min, runs `nit sync --forward-only-paths` whenever any forward-only target changes. Closes the "I forgot to re-add for a month" failure mode. Batches appends to reduce commit churn. Pairs naturally with nit's trigger system.

  **Interim during migration**: just don't deploy these files via `nit update`. Keep them tracked in the bare repo, let `nit commit <path>` capture runtime state when the user chooses. Adds discipline but removes the overwrite risk.

- [x] **✅ DONE Apr 21, 2026: Narrow tier recipients in `fleet.toml`** + rekey: per `docs/age_key_mapping.md`, dropped both unconfirmed Shannon-candidate keys, scoped each tier to its actual access set (tier-all=4, tier-servers=2, tier-mac=2, tier-edge=1). Required two nit fixes en route: (a) age crate `armor` feature wasn't enabled — chezmoi writes ASCII-armored .age files but nit's `Decryptor::new_buffered` expected binary input, failing with "Header is invalid"; fixed by enabling the armor feature + wrapping with `age::armor::ArmoredReader::new(...)` for transparent format detection. (b) `nit rekey` then succeeded and re-encrypted all 4 tiers with narrowed recipients. Verified: all 4 tiers still decrypt to identical plaintext byte counts. Add Shannon's verified key when it's powered on next.

- [x] **✅ DONE Apr 21, 2026: Recover 9 templates + 399 system files** silently dropped from migration commit by `Library/`/`.graphiti/` blanket gitignore matching identically-named subpaths under `dotfiles/`. Fixed gitignore with `!dotfiles/templates/**` + `!dotfiles/system/**`. The lost files were 8 LaunchAgent plist templates, 1 graphiti redis.conf template, ~390 fonts, and ~10 keyboard layout files — all project sources, not runtime state.

- [x] **✅ DONE Apr 21, 2026: `build-ontology` trigger registered** (per Apr 20 plan). New `scripts/darwin/build-ontology.sh` wrapper invokes `~/.claude/hooks/build_ontology.py` (uses hooks venv if present). triggers.toml entry watches `Projects/graphiti-official/mcp_server/{custom_entities.py,graphiti_mcp_server.py}` for changes. Verified: nit list now shows 11 triggers (was 10).

- [x] **✅ DONE Apr 21, 2026: Add `os` field per machine in `fleet.toml`** (informational). nit reads the OS from runtime via `current_os()` (the macos→darwin normalization), but other tools (hemma) may consume the field. Now annotated:
  ```toml
  [machines.macmini]
  ssh_host = "macmini"
  os = "darwin"           # add
  role = ["dev"]

  [machines.darwin]
  ssh_host = "darwin"
  os = "linux"            # add — Dell Optiplex running Ubuntu, name notwithstanding
  role = ["server", "router"]
  critical = true

  [machines.merian]
  ssh_host = "mbp"
  os = "darwin"           # add — MacBook Pro 2014, Big Sur
  role = ["laptop"]

  [machines.turing]
  ssh_host = "turing"
  os = "linux"            # add — Raspberry Pi 3B+, Debian Trixie
  role = ["iot"]

  [machines.shannon]
  ssh_host = "shannon"
  os = "linux"            # add — Rock Pi 4B SE, Armbian Trixie (cold spare)
  role = ["router"]
  critical = true
  ```
  Verify after the edit: `nit list` should show all applicable triggers (10 on macmini, ~9 on darwin/shannon, etc.). This is the same mechanism nit uses for `[trigger] os = "darwin"` filtering — without it nit treats every machine as os-agnostic and drops anything with an os filter.

- [ ] **Investigate `falkordb.plist.tmpl` ✗ in `nit list`** (discovered Apr 21, 2026): Of 11 templates, 10 show ✓ (rendered cleanly, target matches) but `Library/LaunchAgents/com.fredrikbranstrom.falkordb.plist.tmpl` shows ✗. Likely either a Tera conversion error in the converted template or a deliberate target mismatch (FalkorDB primary moved to Darwin Mar 2026; the local plist may be intentionally stopped/disabled — see global CLAUDE.md § "Dev databases on Darwin"). Diagnose:
  ```bash
  nit apply --dry-run 2>&1 | grep -A3 falkordb
  diff <(nit render dotfiles/templates/Library/LaunchAgents/com.fredrikbranstrom.falkordb.plist.tmpl) ~/Library/LaunchAgents/com.fredrikbranstrom.falkordb.plist
  ```

- [x] **✅ DONE Apr 21, 2026: Add `os` field per machine in `fleet.toml`** (informational — nit reads OS from runtime, hemma may use the field). Added `os = "darwin"` for macmini/merian, `os = "linux"` for darwin/turing/shannon.

- [x] **✅ DONE Apr 21, 2026: Investigate `falkordb.plist.tmpl` ✗ in `nit list`** — root cause: Mac Mini deliberately stops local FalkorDB (Darwin runs it now); the deployed file is renamed to `.plist.disabled` by `dev-db-toggle`. Template rendered to `.plist` so target mismatch. **Fix shipped**: renamed source `templates/Library/LaunchAgents/com.fredrikbranstrom.falkordb.plist.tmpl` → `.plist.disabled.tmpl` (matches the existing `graphiti.plist.disabled.tmpl` convention). nit list now shows ✓.

- [x] **✅ INVESTIGATED Apr 21, 2026: tier-servers / tier-mac / tier-edge ✗ in nit list** (root cause identified, not a real problem). The ✗/✓ marker in `nit list`'s secret section is a NAIVE HEURISTIC: `tier_name.contains(my_role) || tier_name.contains("all")`. Macmini has role=["dev"]; tier names contain "all"/"servers"/"mac"/"edge" but not "dev". So tier-all matches via "all" substring and shows ✓; the others show ✗. **The actual decryption works fine** — verified with `age --decrypt -i ~/.config/nit/age-key.txt`, all 4 tier .age files decrypt correctly and content matches deployed plaintexts byte-for-byte.

  **The marker is misleading**, not load-bearing. v2 polish: replace the heuristic with actual age-key recipient check (parse `~/.config/nit/age-key.txt` → derive public key → check membership in `tier.recipients`). ~30 lines of Rust, low priority.

- [x] **✅ FIXED Apr 21, 2026: macOS → darwin OS normalization** (nit bug discovered during cleanup). `std::env::consts::OS` returns `"macos"` on macOS, but trigger files (and chezmoi/Unix uname/shell scripts everywhere) use `"darwin"`. Result: every `[trigger] os = "darwin"` was filtered out on macmini — `nit list` showed 3 triggers instead of 10. Same bug affected templates: `{% if os == "darwin" %}` blocks were never matched.

  Fix shipped: `crate::config::current_os()` helper normalizes "macos" → "darwin" at all comparison points. Used in `applicable_triggers()` and template rendering. Verified: nit list now shows all 10 expected triggers on macmini.

- [ ] **Add `__pycache__/` and `*.pyc` to `~/.gitignore`** (discovered Apr 21, 2026): `~/dotfiles/scripts/__pycache__/ensure_claude_graphiti_mcp.cpython-314.pyc` is pre-existing detritus (Mar 15) — not migration-related but the new gitignore doesn't exclude `__pycache__/`. Either `rm -rf ~/dotfiles/scripts/__pycache__` and add the gitignore line, or just add the exclusion. Trivial fix; lump with any other gitignore tuning.

- [ ] **v2: Drift auto-promotion to source** (deferred design Apr 21, 2026): when target drift is VALUABLE (~50% of cases per spec), the current workflow requires manually editing source. For agents this is fast (paste from diff into source, commit) but two safer-automation enhancements are worth designing and building when the friction emerges:

  **Already shipped Apr 21 (safe ergonomics, no auto-merge)**:
  - `nit pick --diff <file>` → print drift as unified diff to stdout (read-only, no ack write). Pipe-friendly for `git apply` / `patch` chains.
  - `nit pick --edit <file>` → print drift to stderr, open template SOURCE in `$EDITOR`. User incorporates desired changes into the right conditional branch by hand. Writes ack (active review). After save+exit, user runs `nit commit`.

  **Deferred — needs intelligent design**:

  - **`nit pick --apply <file>`** (Strategy D from the design discussion): attempt to git-apply the drift diff onto the template source, with re-render verification.
    ```
    Algorithm:
      1. drift_diff = unified_diff(target_rendered, target_current)
      2. git apply --check drift_diff onto source.tmpl
         (this naturally fails if the diff hunks touch lines that have
         template syntax in source but plain text in rendered — because
         the literal context line in the diff won't match source)
      3. If --check passes:
           a. Apply diff to source
           b. Re-render source → rendered_after
           c. If rendered_after == target_current: SUCCESS — drift promoted
           d. Else: revert source change, FAIL (false-clean apply)
         If --check fails: FAIL — recommend --edit instead
      4. On success: print "drift promoted to source, run nit commit"
                     write ack
    ```
    **Strengths**: handles plain templates and conditionals where drift falls in
    the active branch. Verification via re-rendering catches false positives.
    Refuses to silently break templates whose drift overlaps template syntax.

    **Limitations**: when drift IS in a conditional branch and the user wants the
    change in OTHER branches too, this command only updates the current-machine
    branch. User must check other branches manually. Acceptable trade-off — the
    common case is "drift on this machine, want it on this machine".

  - **`nit pick --apply --with-llm <file>`** (Strategy F, opt-in AI-assisted):
    when `--apply` fails (template syntax overlap, multi-branch reasoning needed),
    invoke a capable LLM with the source template + rendered output + drifted
    target, asking it to produce the source modification that yields the drifted
    rendering. Show the proposed source diff, require user confirm before write.
    Higher quality on complex cases. Always opt-in.

    **Implementation hint**: pipe to `fabric` (already installed in this fleet)
    with a custom pattern like `promote_template_drift` — fabric handles the
    LLM provider abstraction (Claude / GPT / Gemini / local). Keeps nit
    LLM-agnostic and reuses the existing fabric infrastructure rather than
    embedding a Rust HTTP client per provider.
    ```bash
    # Sketch:
    cat <<EOF | fabric -p promote_template_drift
    # SOURCE TEMPLATE
    $(cat dotfiles/templates/.zshenv.tmpl)
    # RENDERED OUTPUT (what nit produced)
    $(nit render .zshenv)
    # DRIFTED TARGET (what's actually on disk)
    $(cat ~/.zshenv)
    EOF
    # Output: proposed source diff, user reviews + confirms
    ```

  - **`nit pick --copy-to-source <file>`**: degenerate case — for templates that
    are PURE LITERAL (no `{% %}` / `{{ }}` syntax at all), copy target_current
    verbatim to source.tmpl. Effectively just file copy with safety check. Could
    be folded into `--apply` (which handles this case naturally — diff applies
    cleanly, re-render matches).

  **When to build**: when manual edit-source flow becomes painful in practice. For
  AI-agent primary use case (paste drift into source via editor), the manual flow
  is ~30 seconds. Worth measuring real-world friction before adding complexity.

- [x] **✅ FIXED Apr 21, 2026: Per-PPID ack persistence** (was a 🔴 BLOCKER for serious nit usage). The commit ack system originally tied acks to `getppid()`, which broke for any workflow with ephemeral child shells (CC's Bash tool, scripted invocations, CI runs). Each invocation had a different PPID — the ack written on attempt N never persisted to attempt N+1.

  **Fix shipped**: replaced raw `getppid()` with `get_session_anchor()` — walks the parent process chain and stops at the first KNOWN_AGENTS process (claude, codex, cursor-agent, aider, opencode, amp) or KNOWN_BOUNDARIES process (ghostty, kitty, alacritty, iTerm, wezterm, warp, tmux, screen, zellij, sshd, login, launchd, systemd, init, cron, crond). Returns the agent's PID OR the topmost shell's PID, whichever is found first. Within one CC conversation, all Bash calls now share the same anchor (the claude process PID).

  **Cross-session ack reuse simultaneously removed** — the original "Option A" optimization defended none of the named contamination incidents (sccache, Flux client revert, CLAUDE.md re-add overwrite are all defended by source-wins + no-auto-merge), and per-agent accountability is cleaner. See `dotfiles/.claude/specs/nit/design.md` § "Why no cross-session ack reuse" for the full rationale.

  **Verified**: this CC session triggered `nit pick`, ack landed at `<claude-pid>.json` not `<ephemeral-bash-pid>.json`. All Bash calls in the same conversation now find each other's acks correctly.

  **Long-term improvement** (separate work item): convince agentic harnesses to set a standard `AGENT_SESSION_ID` env var so the KNOWN_AGENTS heuristic isn't needed. Until then, the list is extensible — one entry per harness.

- [x] **Per-machine binaries strategy** (audit Apr 21, 2026; mostly resolved same day). The migration revealed many large binaries at `~/.local/bin/` that aren't (and shouldn't be) tracked by nit. Audit + outcome:

  | Binary | Size | Source / Install method | Covered? |
  |---|---|---|---|
  | mise | 66M | `mise.run/install.sh` self-installer | ✅ `scripts/27-setup-mise.sh` bootstraps via `curl mise.run` when missing |
  | uv | 39M | `mise install` (mise tool) | ✅ via `~/.config/mise/config.toml` |
  | fabric | 41M | `go install ...@latest` (Monterey+) or `@v1.4.311` (Big Sur, last Go 1.24 release) | ✅ `scripts/darwin/install-extra-binaries.sh` with `sw_vers` branch for Big Sur |
  | project-launcher-tui | 7.8M | local go build from `~/.local/src/project-launcher` | ✅ install-extra-binaries |
  | project-registry | 1.7M | `cargo install --git git@github.com:semikolon/project-registry` | ✅ install-extra-binaries |
  | system-sentinel | 1.4M | cargo install from semikolon/system-sentinel (local or git) | ✅ install-extra-binaries |
  | ccsearch | 2.5M | cargo install from semikolon/ccsearch (local or git) | ✅ install-extra-binaries |
  | dcg | 14M | `cargo install --git Dicklesworthstone/destructive_command_guard` | ✅ install-extra-binaries |
  | mcp-agent-mail | 49M | (unclear crate source; v0.2.3) | ❌ DELIBERATELY deferred — usage unclear |
  | am | 49M | (unclear crate source; v0.2.3; likely antigravity-related) | ❌ DELIBERATELY deferred — usage unclear |
  | notification-reader | 2.1M | `dotfiles/scripts/25-build-rust-hooks.sh` | ✅ (trigger, always was) |
  | tts-cli | 1.0M | `dotfiles/scripts/25-build-rust-hooks.sh` | ✅ (trigger, always was) |
  | narrate-client, context-nudge, skill-preeval | various | `dotfiles/scripts/25-build-rust-hooks.sh` | ✅ (trigger, always was) |

  **Outcome**: 11 of 13 binaries now have install automation on a fresh machine. `am` and `mcp-agent-mail` remain deliberately manual (per user decision — usage unclear, defer until explicit need surfaces). The Big Sur fabric branch was added Apr 21 after verifying v1.4.311 is the last fabric release with `go 1.24.0` in go.mod; v1.4.312+ require Go 1.25 which won't run on macOS 11. Prebuilt `fabric_Darwin_x86_64.tar.gz` binaries also fail on Big Sur (link against `_SecTrustCopyCertificateChain`, a macOS 12 symbol).

- [ ] **Add `build_ontology` trigger to `triggers.toml`** (Apr 20, 2026): `~/.claude/contextual-intelligence/ontology.json` is consumed by `/capture`, `/significance`, and `/total-recap` as pre-command context. Its source-of-truth is two files in the separately-managed graphiti-official fork — NOT in nit's tracked tree. Currently manual rebuild (`python3 ~/.claude/hooks/build_ontology.py`). After nit lands, add a trigger that watches both files and rebuilds on hash change. Proposed stanza (watch paths are `$HOME`-relative — graphiti-official lives under `~/Projects/` which is $HOME-relative, so no absolute paths needed):
  ```toml
  [[trigger]]
  name = "build-ontology"
  script = "scripts/darwin/build-ontology.sh"   # or a direct python3 invocation
  watch = [
    "Projects/graphiti-official/mcp_server/custom_entities.py",
    "Projects/graphiti-official/mcp_server/graphiti_mcp_server.py",
  ]
  os = "darwin"  # only needed where Claude Code runs
  ```
  The wrapper script just runs `python3 ~/.claude/hooks/build_ontology.py`. The script is safe to run repeatedly — idempotent, ~150ms, refreshes both `ontology.json` and `entity_types_fallback.json`. Context + design rationale: `dotfiles/docs/household_cognitive_infrastructure_plan_2026_04_20.md` § "Actions — landed today" (entity-type additions) and the follow-up note about manual rebuild.

## Fleet nit rollout mechanism (shipped Apr 21, 2026)

Post-migration open loop: *how does a new nit version reach fleet machines?* Previously manual `cargo install --git ... --root ~/.local` per machine, creating silent version drift. Shipped solution (Apr 21) uses nit's own primitives to solve this.

**Architecture**:

1. **`build.rs`** (in nit) bakes the git SHA into the binary via `NIT_GIT_SHA` env var. Clap's `version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("NIT_GIT_SHA"), ")")` exposes it. `nit --version` → `nit 0.1.0 (f5d69554e8d9d21f091cd78b82e58ccbd41e10c5)`. Falls back to "unknown" when built outside a git checkout (crates.io packaging, tarball builds).

2. **`dotfiles/.nit-version`** (plain file, `$HOME/dotfiles/.nit-version`, nit-tracked): contains the SHA the fleet should run. Comments allowed (first non-blank, non-comment line wins). Git log on this file is the release history.

3. **`dotfiles/scripts/rebuild-nit.sh`** (cross-OS trigger script): reads `.nit-version`, parses installed `nit --version`, compares. If match → skip. If differ (or installed SHA is "unknown") → `cargo install --git https://github.com/semikolon/nit --rev <sha> --root ~/.local --force`. Graceful fallback: skip if cargo missing or `.nit-version` missing.

4. **`triggers.toml`** entry (first in the file so it runs first in the trigger pass):

   ```toml
   [[trigger]]
   name = "rebuild-nit"
   script = "scripts/rebuild-nit.sh"
   watch = ["dotfiles/.nit-version"]
   ```

   No OS or role filter — any machine with nit (all of them) and cargo (all dev + router machines) runs it. Non-cargo machines exit 0 gracefully.

**Release flow**:

```
# 1. Push the new nit commit
cd ~/Projects/nit && git push origin master

# 2. Bump the fleet pin
cd ~/dotfiles
echo "# comment" > .nit-version
git -C ~/Projects/nit rev-parse HEAD >> .nit-version  # or a specific SHA
nit add dotfiles/.nit-version
nit commit -m "release: nit @<short-sha>"

# 3. Each fleet machine catches up on its next `nit update`:
#    - pull picks up the new .nit-version
#    - rebuild-nit trigger fires (watched file hash changed)
#    - cargo install --force brings the binary to the pinned SHA
#    - NEXT `nit update` uses the new binary
```

**Sacred drift-safety compliance**:

- nit never silently replaces itself. The `.nit-version` bump is an explicit, reviewable commit — same contract as any other dotfile change.
- If `cargo install` fails (network, compile, dep conflict), the trigger reports failure via nit's standard `TriggerRunResult::Failed` path. `nit status` / `hemma status` surface it. State is NOT updated on failure, so the retry happens on next cycle.
- The CURRENT `nit update` cycle continues using the binary it was invoked with. The rebuilt binary takes effect on the NEXT invocation. No mid-execution binary swap.

**Rejected alternatives**:

- **Silent auto-update on every `nit update`** — fails sacred drift-safety. A buggy nit could brick all fleet machines simultaneously; recovery would require SSHing into each and running manual cargo install. The `.nit-version` bump gate makes the user's release action explicit and reversible (`git revert`).
- **Notify-only** (show a version-drift warning, no install) — leaves actual upgrade manual per-machine. Loses the automation benefit that was the motivating concern.
- **Semver tags on nit** — extra process (tag management, release branches) without corresponding benefit for a personal fleet. SHA pinning works fine.

**What ships**:

- `nit` commit `f5d6955` (build.rs + SHA-embedded version string)
- `dotfiles` (`.nit-version` file, `scripts/rebuild-nit.sh`, trigger entry, doc updates)
- Initial `.nit-version` pin: `f5d69554e8d9d21f091cd78b82e58ccbd41e10c5`
- Live-verified on Mac Mini: rebuild-nit trigger fired, detected match with pin, skipped. Subsequent `nit apply` confirmed idempotent (trigger shows as Unchanged, no re-fire).
