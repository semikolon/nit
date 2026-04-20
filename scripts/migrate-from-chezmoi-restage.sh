#!/usr/bin/env bash
#
# migrate-from-chezmoi-restage.sh
#
# Step 2 of the chezmoi → nit migration: restructure the bare repo's git
# index so that tracked paths match the new nit layout.
#
# Run AFTER `migrate-from-chezmoi.sh --execute` has completed (which creates
# the bare repo at ~/.local/share/nit/repo.git, moves templates/secrets/scripts
# to ~/dotfiles/{templates,secrets,scripts}/, and writes ~/.gitignore).
#
# WHAT THIS DOES:
#   1. Bump diff/merge.renameLimit (default 1000 < typical chezmoi tree).
#   2. Add gitignore patterns for chezmoi vestiges (so bulk staging is safe).
#   3. Untrack chezmoi source paths (home/dot_*, home/private_*, etc.) and
#      chezmoi metadata (.chezmoiroot, .chezmoi.toml.tmpl, .chezmoiignore,
#      .chezmoi-commit-message.tmpl).
#   4. Untrack and re-stage the dotfiles repo's OWN state files (the ones
#      that lived at ~/dotfiles/<X> rather than under home/) at the new
#      dotfiles/<X> paths now that the bare repo's work tree is $HOME.
#   5. Stage chezmoi-deployed targets at their real $HOME paths.
#   6. Stage moved templates/secrets/scripts at their new dotfiles/ paths
#      and the new nit configs (fleet.toml, triggers.toml, ~/.gitignore).
#   7. Single commit with rename detection enabled.
#
# WHY ONE BIG COMMIT:
#   Git's rename detection runs intra-commit. If we untrack home/dot_zshrc
#   in commit A and add .zshrc in commit B, git can't follow the rename and
#   `git log --follow .zshrc` breaks. All untrack+restage pairs must land in
#   the same commit per file class. One commit is simpler than splitting.
#
# WHY THIS IS SAFE:
#   All operations are index-only — on-disk file content never changes.
#   The pre-nit tag (chezmoi-final too) preserves the entire pre-migration
#   state. Until pushed, every step is reversible via `nit reset HEAD`.
#
# DRY-RUN by default. Use --execute to actually perform the operations.
#
# Usage:
#   ./migrate-from-chezmoi-restage.sh           # dry-run
#   ./migrate-from-chezmoi-restage.sh --execute # do it
#   ./migrate-from-chezmoi-restage.sh --execute --yes  # skip confirms

set -euo pipefail

# ─── Configuration ────────────────────────────────────────────────────
HOME_DIR="${HOME}"
DOTFILES_DIR="${HOME_DIR}/dotfiles"
NIT_REPO="${HOME_DIR}/.local/share/nit/repo.git"
GITIGNORE="${HOME_DIR}/.gitignore"
CHEZMOI_SOURCE="${DOTFILES_DIR}/home"

# ─── Argument parsing ─────────────────────────────────────────────────
DRY_RUN=true
AUTO_YES=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --execute) DRY_RUN=false ;;
        --yes) AUTO_YES=true ;;
        --help|-h)
            sed -n '2,50p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
    shift
done

# ─── Pretty printing ──────────────────────────────────────────────────
RED='\033[31m'; GREEN='\033[32m'; YELLOW='\033[33m'; CYAN='\033[36m'; BOLD='\033[1m'; RESET='\033[0m'
info() { printf "${CYAN}restage${RESET}: %s\n" "$*"; }
ok()   { printf "  ${GREEN}✓${RESET} %s\n" "$*"; }
warn() { printf "  ${YELLOW}⚠${RESET} %s\n" "$*"; }
err()  { printf "  ${RED}✗${RESET} %s\n" "$*" >&2; }
dry()  { printf "  ${CYAN}[dry-run]${RESET} %s\n" "$*"; }

# ─── Pre-flight checks ────────────────────────────────────────────────
[[ -d "$NIT_REPO" ]]        || { err "bare repo not found at $NIT_REPO — run migrate-from-chezmoi.sh --execute first"; exit 1; }
[[ -d "$CHEZMOI_SOURCE" ]]  || { err "chezmoi source not found at $CHEZMOI_SOURCE"; exit 1; }
[[ -f "$GITIGNORE" ]]       || { err "$GITIGNORE not found — run migrate-from-chezmoi.sh first"; exit 1; }
command -v nit >/dev/null   || { err "nit binary not on PATH — install via cargo install --path ~/Projects/nit"; exit 1; }

# Verify bare repo is healthy
HEAD_HASH=$(git --git-dir="$NIT_REPO" rev-parse HEAD 2>/dev/null) || { err "bare repo has no HEAD — corrupted?"; exit 1; }
CURRENT_BRANCH=$(git --git-dir="$NIT_REPO" branch --show-current)
[[ "$CURRENT_BRANCH" == "nit" ]] || { warn "expected branch 'nit', currently on '$CURRENT_BRANCH'"; }

# Move cwd OUTSIDE the bare repo's work tree ($HOME) so git operations
# don't get implicit pathspec filtering. From inside ~/dotfiles/ cwd,
# `git ls-tree HEAD` returns 0 entries because it filters to "tree under
# current subpath" which is empty in the new layout.
cd /

# ─── Mode banner ──────────────────────────────────────────────────────
if $DRY_RUN; then
    printf "\n${BOLD}${CYAN}chezmoi → nit restage — DRY RUN${RESET}\n"
    printf "Showing what would happen. Run with --execute to perform.\n\n"
else
    printf "\n${BOLD}${CYAN}chezmoi → nit restage — EXECUTE MODE${RESET}\n"
    printf "Will modify the bare repo index and create one big commit.\n"
    printf "  bare repo: %s\n" "$NIT_REPO"
    printf "  HEAD:      %s\n" "$HEAD_HASH"
    printf "  branch:    %s\n\n" "$CURRENT_BRANCH"
    if ! $AUTO_YES; then
        printf "Continue? [y/N] "
        read -r confirm
        [[ "$confirm" == "y" || "$confirm" == "Y" ]] || { warn "aborted"; exit 1; }
    fi
fi

# ─── Helper: nit fall-through with dry-run support ────────────────────
nit_run() {
    if $DRY_RUN; then
        dry "nit $*"
    else
        # nit's external_subcommand falls through to git with bare strategy
        nit "$@" 2>&1 | sed 's/^/    /'
    fi
}

# ─── Phase A: Bump rename detection limits ────────────────────────────
info "Phase A: Bumping rename detection limits (default 1000 too low for chezmoi trees)"
if $DRY_RUN; then
    dry "git config diff.renameLimit 5000 (in $NIT_REPO)"
    dry "git config merge.renameLimit 5000 (in $NIT_REPO)"
else
    git --git-dir="$NIT_REPO" config diff.renameLimit 5000
    git --git-dir="$NIT_REPO" config merge.renameLimit 5000
    ok "diff.renameLimit + merge.renameLimit = 5000"
fi

# ─── Phase B: Final gitignore additions ───────────────────────────────
# Why: bulk `nit add dotfiles/` would walk into ~/dotfiles/home/ (chezmoi
# source about to be deleted) and re-stage 590+ files at dotfiles/home/dot_*
# paths — exactly the inverse of what we want. Same for vestigial root
# dotfiles at ~/dotfiles/{.zshrc,.bash_profile,...}.
info "Phase B: Ensuring ~/.gitignore covers chezmoi source + vestiges"

PHASE_B_PATTERNS=(
    "# ─── Phase B (chezmoi vestiges) ─────────────────────────────────"
    "# Chezmoi source — about to be deleted in Phase 11 of migration"
    "dotfiles/home/"
    ""
    "# Vestigial pre-chezmoi root files at ~/dotfiles/ (Dec 2024 leftovers,"
    "# not deployed anywhere; chezmoi-deployed copies tracked at \$HOME paths)"
    "dotfiles/.bash_profile"
    "dotfiles/.gitconfig"
    "dotfiles/.gitignore"
    "dotfiles/.gitignore_global"
    "dotfiles/.profile"
    "dotfiles/.tcshrc"
    "dotfiles/.vimrc"
    "dotfiles/.zshenv"
    "dotfiles/.zshrc"
    ""
    "# Whitelist non-dot \$HOME paths chezmoi tracked (overrides /[!.]*)"
    "!Documents/"
    "Documents/*"
    "!Documents/superwhisper/"
)

if grep -qF "Phase B (chezmoi vestiges)" "$GITIGNORE"; then
    ok "gitignore already has Phase B additions"
else
    if $DRY_RUN; then
        dry "append ${#PHASE_B_PATTERNS[@]} lines to $GITIGNORE (chezmoi vestiges + Documents whitelist)"
    else
        printf "\n" >> "$GITIGNORE"
        printf '%s\n' "${PHASE_B_PATTERNS[@]}" >> "$GITIGNORE"
        ok "appended Phase B patterns to ~/.gitignore"
    fi
fi

# ─── Phase C: Untrack chezmoi source + metadata ───────────────────────
info "Phase C: Untracking chezmoi source (home/) and metadata"
nit_run rm -r --cached home/
nit_run rm --cached .chezmoiroot 2>/dev/null || true
# .chezmoi.toml.tmpl, .chezmoiignore, .chezmoi-commit-message.tmpl all live
# under home/ in our setup so are already removed by the recursive rm above.
# (If your chezmoi setup has them at the repo root, add explicit rm calls.)

# ─── Phase D: Re-stage dotfiles project state at dotfiles/<X> ─────────
# These are files/dirs the dotfiles repo tracked at root — not chezmoi
# source, but the project's own state (specs, docs, system overlays, etc.).
# After bare-repo move with core.worktree=$HOME, paths like .claude/specs
# resolve to ~/.claude/specs (chezmoi-deployed CC config) rather than
# ~/dotfiles/.claude/specs (where they actually live). Re-stage at the
# correct dotfiles/<X> path.
info "Phase D: Re-staging dotfiles project state at dotfiles/<X> paths"

# Vestiges to untrack-only (don't re-stage at dotfiles/<X>)
VESTIGE_FILES=(
    .bash_profile .gitconfig .gitignore .gitignore_global
    .profile .tcshrc .vimrc .zshenv .zshrc
)
is_vestige() {
    local entry="$1"
    for v in "${VESTIGE_FILES[@]}"; do
        [[ "$entry" == "$v" ]] && return 0
    done
    return 1
}

# Paths to skip entirely (handled in Phase C, or explicit skip)
should_skip() {
    case "$1" in
        home|.chezmoiroot|.chezmoi*) return 0 ;;  # handled in Phase C
    esac
    return 1
}

# Enumerate all root-tracked entries
ROOT_ENTRIES=()
while IFS= read -r entry; do
    ROOT_ENTRIES+=("$entry")
done < <(git --git-dir="$NIT_REPO" ls-tree HEAD --name-only)

restaged=0
vestiged=0
skipped=0
for entry in "${ROOT_ENTRIES[@]}"; do
    if should_skip "$entry"; then
        skipped=$((skipped+1))
        continue
    fi
    if is_vestige "$entry"; then
        # Untrack only, don't re-stage (gitignore covers re-add prevention)
        nit_run rm --cached "$entry" 2>/dev/null || true
        vestiged=$((vestiged+1))
        continue
    fi
    # Untrack at root path + re-stage at dotfiles/<X>
    if [[ -d "${DOTFILES_DIR}/${entry}" ]]; then
        nit_run rm -r --cached "$entry" 2>/dev/null || true
        nit_run add "dotfiles/${entry}"
    elif [[ -e "${DOTFILES_DIR}/${entry}" ]]; then
        nit_run rm --cached "$entry" 2>/dev/null || true
        nit_run add "dotfiles/${entry}"
    else
        warn "tracked entry '$entry' not found at ${DOTFILES_DIR}/${entry} — skipping"
        skipped=$((skipped+1))
        continue
    fi
    restaged=$((restaged+1))
done
ok "Phase D: re-staged $restaged, untracked-only $vestiged vestiges, skipped $skipped"

# ─── Phase E: Stage chezmoi-deployed targets at $HOME ─────────────────
# Walk the chezmoi source tree, derive deployed top-level names, stage
# each one at its $HOME path. nit add walks recursively; gitignore filters
# out runtime/build artifacts.
info "Phase E: Staging chezmoi-deployed targets at \$HOME paths"

resolve_chezmoi_top() {
    # Convert a top-level chezmoi source name to its deployed name.
    # Only handles the leading prefix; nested chezmoi prefixes inside
    # the dir are deployed to their on-disk paths already.
    local name="$1"
    case "$name" in
        encrypted_private_*) echo "${name#encrypted_private_}" ;;
        private_dot_*)       echo ".${name#private_dot_}" ;;
        private_*)           echo "${name#private_}" ;;
        dot_*)               echo ".${name#dot_}" ;;
        executable_*)        echo "${name#executable_}" ;;
        symlink_*)           echo "${name#symlink_}" ;;
        *)                   echo "$name" ;;
    esac
}

DEPLOYED_TOPLEVELS=()
# Both globs needed — bash * doesn't match dotfiles by default
for src in "$CHEZMOI_SOURCE"/* "$CHEZMOI_SOURCE"/.[!.]*; do
    [[ -e "$src" ]] || continue
    name="$(basename "$src")"
    # Skip leading-dot entries at chezmoi source root: chezmoi treats
    # these as metadata (.chezmoi*, .DS_Store) or vestiges
    # (.commit-template.txt that was never deployed because of the dot
    # prefix). Real chezmoi sources use dot_X / private_dot_X / etc.
    case "$name" in
        .*) continue ;;
    esac
    # Skip top-level templates — they're moved to dotfiles/templates/
    # by the main migration script (Phase F here stages that location);
    # the rendered output (~/.zprofile, ~/.zshenv) is regenerated on
    # nit apply so it doesn't need separate tracking.
    [[ "$name" == *.tmpl ]] && continue
    deployed=$(resolve_chezmoi_top "$name")
    DEPLOYED_TOPLEVELS+=("$deployed")
done

staged=0
missing=0
for entry in "${DEPLOYED_TOPLEVELS[@]}"; do
    if [[ -e "${HOME_DIR}/${entry}" || -L "${HOME_DIR}/${entry}" ]]; then
        nit_run add "$entry"
        staged=$((staged+1))
    else
        warn "deployed target ~/$entry not found on disk — skipping"
        missing=$((missing+1))
    fi
done
ok "Phase E: staged $staged \$HOME targets (missing: $missing)"

# ─── Phase F: Stage moved configs + new nit configs ───────────────────
info "Phase F: Staging moved configs + new nit configs"
for path in dotfiles/templates dotfiles/secrets dotfiles/scripts dotfiles/fleet.toml dotfiles/triggers.toml .gitignore; do
    if [[ -e "${HOME_DIR}/${path}" ]]; then
        nit_run add "$path"
    else
        warn "expected $path not found — skipping"
    fi
done

# ─── Phase G: Summary + commit ────────────────────────────────────────
info "Phase G: Summary"
if $DRY_RUN; then
    dry "would run: nit status"
    dry "would commit single big restructure commit with rename detection"
    printf "\n${BOLD}DRY RUN COMPLETE${RESET} — no changes made.\n"
    printf "Run with ${BOLD}--execute${RESET} to perform the restage.\n"
    exit 0
fi

printf "\n${BOLD}nit status (post-staging):${RESET}\n"
nit status 2>&1 | head -20

if ! $AUTO_YES; then
    printf "\n${BOLD}Commit now? [y/N] ${RESET}"
    read -r confirm
    [[ "$confirm" == "y" || "$confirm" == "Y" ]] || {
        warn "aborted (index has all stages); reset with: nit reset HEAD"
        exit 1
    }
fi

COMMIT_MSG="nit: restructure from chezmoi source layout to nit layout

Untracked chezmoi source (home/dot_*, private_*, encrypted_*) — replaced by
direct tracking of deployed dotfiles at \$HOME paths.

Untracked chezmoi metadata (.chezmoiroot, .chezmoi.toml.tmpl, .chezmoiignore,
.chezmoi-commit-message.tmpl).

Re-staged dotfiles repo's own project state (.claude/, docs/, hemma/, system/,
tests/, etc.) at dotfiles/<X> paths now that bare repo work-tree is \$HOME.

Staged moved templates, secrets, scripts at new dotfiles/{templates,secrets,
scripts}/ locations + new nit configs (fleet.toml, triggers.toml, ~/.gitignore).

Plain files: R100 renames detected (content unchanged).
Templates: similarity-based renames (Go→Tera conversion preserved most logic).
Secrets: R100 renames (encrypted blob unchanged).
"

if nit commit -m "$COMMIT_MSG"; then
    ok "commit created"
    NEW_HEAD=$(git --git-dir="$NIT_REPO" rev-parse HEAD)
    RENAMES=$(git --git-dir="$NIT_REPO" log --stat HEAD~1..HEAD 2>/dev/null | grep -c "^ rename" || true)
    info "New HEAD: $NEW_HEAD"
    info "Rename detection: $RENAMES renames detected"
    if [[ "$RENAMES" -lt 100 ]]; then
        warn "rename count looks low — verify with: nit log --stat HEAD~1..HEAD"
    fi
else
    err "commit failed — index left in staged state"
    err "to reset: nit reset HEAD"
    exit 1
fi

ok "Phase 6 restage complete. Next: nit pick / nit apply to verify, then nit push."
