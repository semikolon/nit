#!/usr/bin/env bash
# migrate-from-chezmoi.sh — Convert a chezmoi-managed dotfiles repo to nit
#
# This script converts ~/dotfiles/home/ (chezmoi source) into a nit-compatible
# structure where plain files live directly in $HOME (tracked by bare git) and
# only templates/secrets/triggers remain in ~/dotfiles/.
#
# SAFE BY DEFAULT: runs in --dry-run mode unless --execute is passed.
# Creates a 'nit' branch — master stays chezmoi-compatible as rollback.
#
# Usage:
#   ./migrate-from-chezmoi.sh                  # dry-run (show what would happen)
#   ./migrate-from-chezmoi.sh --execute        # actually perform the migration
#   ./migrate-from-chezmoi.sh --execute --yes  # skip confirmation prompt

set -euo pipefail

# ─── Configuration ────────────────────────────────────────────────────────────
DOTFILES_DIR="$HOME/dotfiles"
CHEZMOI_SOURCE="$DOTFILES_DIR/home"
NIT_TEMPLATES="$DOTFILES_DIR/templates"
NIT_SECRETS="$DOTFILES_DIR/secrets"
NIT_SCRIPTS="$DOTFILES_DIR/scripts"
NIT_REPO="$HOME/.local/share/nit/repo.git"
HOME_DIR="$HOME"

DRY_RUN=true
AUTO_YES=false

for arg in "$@"; do
    case "$arg" in
        --execute) DRY_RUN=false ;;
        --yes) AUTO_YES=true ;;
        --help|-h)
            echo "Usage: $0 [--execute] [--yes]"
            echo "  --execute  Actually perform the migration (default: dry-run)"
            echo "  --yes      Skip confirmation prompt"
            exit 0
            ;;
    esac
done

# ─── Colors ───────────────────────────────────────────────────────────────────
GREEN='\033[32m'
YELLOW='\033[33m'
CYAN='\033[36m'
RED='\033[31m'
BOLD='\033[1m'
RESET='\033[0m'

info()  { printf "${CYAN}nit-migrate${RESET}: %s\n" "$1"; }
ok()    { printf "  ${GREEN}✓${RESET} %s\n" "$1"; }
skip()  { printf "  ${YELLOW}⊘${RESET} %s\n" "$1"; }
warn()  { printf "  ${YELLOW}⚠${RESET} %s\n" "$1"; }
err()   { printf "  ${RED}✗${RESET} %s\n" "$1" >&2; }
dry()   { printf "  ${CYAN}[dry-run]${RESET} %s\n" "$1"; }

# ─── Counters ─────────────────────────────────────────────────────────────────
plain_count=0
template_count=0
secret_count=0
script_count=0
symlink_count=0
skip_count=0

# ─── Pre-flight checks ───────────────────────────────────────────────────────
if [ ! -d "$CHEZMOI_SOURCE" ]; then
    err "chezmoi source not found at $CHEZMOI_SOURCE"
    exit 1
fi

if [ ! -f "$DOTFILES_DIR/.chezmoiroot" ]; then
    err "not a chezmoi repo (no .chezmoiroot)"
    exit 1
fi

if $DRY_RUN; then
    printf "\n${BOLD}${CYAN}nit migration — DRY RUN${RESET}\n"
    printf "Showing what would happen. Run with --execute to perform.\n\n"
else
    printf "\n${BOLD}${CYAN}nit migration — EXECUTE MODE${RESET}\n"
    if ! $AUTO_YES; then
        printf "${YELLOW}This will restructure your dotfiles repo.${RESET}\n"
        printf "A 'nit' branch will be created. master stays as rollback.\n"
        printf "Continue? [y/N] "
        read -r confirm
        if [ "$confirm" != "y" ] && [ "$confirm" != "Y" ]; then
            printf "Aborted.\n"
            exit 0
        fi
    fi
    printf "\n"
fi

# ─── Phase 0: Create nit branch ──────────────────────────────────────────────
info "Phase 0: Branch setup"
if $DRY_RUN; then
    dry "git checkout -b nit (from current HEAD)"
else
    cd "$DOTFILES_DIR"
    if git branch --list nit | grep -q nit; then
        warn "branch 'nit' already exists — checking out"
        git checkout nit
    else
        git checkout -b nit
        ok "created and checked out 'nit' branch"
    fi
fi

# ─── Phase 1: Categorize all chezmoi source files ────────────────────────────
info "Phase 1: Categorizing chezmoi source files"

# Arrays for each category
declare -a PLAIN_FILES=()      # Regular files → move to $HOME
declare -a TEMPLATE_FILES=()   # .tmpl files → move to templates/
declare -a SECRET_FILES=()     # .age files → move to secrets/
declare -a SCRIPT_FILES=()     # run_onchange_after_* → move to scripts/
declare -a SYMLINK_FILES=()    # symlink_* → convert to real symlinks
declare -a SKIP_FILES=()       # chezmoi metadata, ignore

# Walk chezmoi source
while IFS= read -r src_file; do
    # Relative to chezmoi source dir
    rel="${src_file#$CHEZMOI_SOURCE/}"
    basename_file="$(basename "$rel")"
    dirname_file="$(dirname "$rel")"

    # Skip chezmoi metadata files
    case "$basename_file" in
        .chezmoiignore|.chezmoi.toml.tmpl|.chezmoi-commit-message.tmpl|.DS_Store)
            SKIP_FILES+=("$rel")
            continue
            ;;
    esac

    # Skip chezmoi scripts directory entirely (handled separately)
    case "$rel" in
        .chezmoiscripts/*)
            # These become trigger scripts
            SCRIPT_FILES+=("$rel")
            script_count=$((script_count + 1))
            continue
            ;;
    esac

    # Encrypted files (.age) → secrets
    case "$basename_file" in
        *.age)
            SECRET_FILES+=("$rel")
            secret_count=$((secret_count + 1))
            continue
            ;;
    esac

    # Symlink files → real symlinks
    case "$basename_file" in
        symlink_*)
            SYMLINK_FILES+=("$rel")
            symlink_count=$((symlink_count + 1))
            continue
            ;;
    esac

    # Template files (.tmpl) → templates/ (but not chezmoi metadata)
    case "$basename_file" in
        *.tmpl)
            TEMPLATE_FILES+=("$rel")
            template_count=$((template_count + 1))
            continue
            ;;
    esac

    # Everything else → plain file
    PLAIN_FILES+=("$rel")
    plain_count=$((plain_count + 1))

done < <(find "$CHEZMOI_SOURCE" -type f ! -path '*/target/*' ! -name '*.o' ! -name '*.d' ! -name '*.rlib' ! -name '*.rmeta')

printf "\n  Summary:\n"
printf "    Plain files:  %d (move to \$HOME)\n" "$plain_count"
printf "    Templates:    %d (move to templates/)\n" "$template_count"
printf "    Secrets:      %d (move to secrets/)\n" "$secret_count"
printf "    Scripts:      %d (move to scripts/)\n" "$script_count"
printf "    Symlinks:     %d (convert to real symlinks)\n" "$symlink_count"
printf "    Metadata:     %d (skip)\n" "${#SKIP_FILES[@]}"
printf "\n"

# ─── Phase 2: Resolve chezmoi prefixes ───────────────────────────────────────
info "Phase 2: Resolving chezmoi naming conventions"

# Resolve a chezmoi source path to a target path
# Strips: dot_, private_dot_, private_, executable_, encrypted_private_
resolve_chezmoi_path() {
    local rel="$1"
    local result=""

    # Process each path component
    IFS='/' read -ra parts <<< "$rel"
    for part in "${parts[@]}"; do
        local resolved="$part"

        # Strip prefixes in order (most specific first)
        # encrypted_private_dot_ → .
        resolved="${resolved#encrypted_private_dot_}"
        if [ "$resolved" != "$part" ]; then
            resolved=".$resolved"
        else
            # encrypted_private_ → (keep name)
            resolved="${resolved#encrypted_private_}"
            if [ "$resolved" = "$part" ]; then
                # private_dot_ → .
                resolved="${resolved#private_dot_}"
                if [ "$resolved" != "$part" ]; then
                    resolved=".$resolved"
                else
                    # private_ → (keep name, just strips prefix)
                    local after_private="${resolved#private_}"
                    if [ "$after_private" != "$resolved" ]; then
                        resolved="$after_private"
                    else
                        # dot_ → .
                        resolved="${resolved#dot_}"
                        if [ "$resolved" != "$part" ]; then
                            resolved=".$resolved"
                        fi
                    fi
                fi
            fi
        fi

        # Strip executable_ prefix (doesn't change the name, just chmod)
        resolved="${resolved#executable_}"

        # Strip symlink_ prefix
        resolved="${resolved#symlink_}"

        if [ -z "$result" ]; then
            result="$resolved"
        else
            result="$result/$resolved"
        fi
    done

    echo "$result"
}

# Test a few resolutions
printf "  Prefix resolution examples:\n"
for test_path in "dot_zshrc" "private_dot_secrets/encrypted_private_tier-all.env.age" "dot_claude/hooks/symlink_stop.sh" "private_Library/LaunchAgents/com.example.daemon.plist.tmpl" "dot_local/bin/executable_my-tool"; do
    resolved=$(resolve_chezmoi_path "$test_path")
    printf "    %s → %s\n" "$test_path" "$resolved"
done
printf "\n"

# ─── Phase 3: Plan file movements ────────────────────────────────────────────
info "Phase 3: Planning file movements"

# Plain files: chezmoi source → $HOME target
printf "\n  ${BOLD}Plain files (→ \$HOME):${RESET}\n"
shown=0
for rel in "${PLAIN_FILES[@]}"; do
    target=$(resolve_chezmoi_path "$rel")
    target_path="$HOME_DIR/$target"
    if $DRY_RUN && [ "$shown" -lt 10 ]; then
        dry "$CHEZMOI_SOURCE/$rel → $target_path"
        shown=$((shown + 1))
    fi
done
if [ "$plain_count" -gt 10 ]; then
    printf "    ... and %d more\n" "$((plain_count - 10))"
fi

# Templates: chezmoi source → templates/ (strip .tmpl for target mapping)
printf "\n  ${BOLD}Templates (→ templates/):${RESET}\n"
for rel in "${TEMPLATE_FILES[@]}"; do
    target=$(resolve_chezmoi_path "$rel")
    # Keep .tmpl extension in templates dir
    printf "    %s → templates/%s\n" "$rel" "$target"
done

# Secrets: chezmoi source → secrets/
printf "\n  ${BOLD}Secrets (→ secrets/):${RESET}\n"
for rel in "${SECRET_FILES[@]}"; do
    target=$(resolve_chezmoi_path "$rel")
    # Extract tier name from filename
    tier_name=$(echo "$target" | sed 's/.*\///' | sed 's/\.env\.age$//')
    printf "    %s → secrets/%s.env.age\n" "$rel" "$tier_name"
done

# Symlinks: chezmoi symlink_ → real symlinks
printf "\n  ${BOLD}Symlinks (→ real symlinks):${RESET}\n"
for rel in "${SYMLINK_FILES[@]}"; do
    target=$(resolve_chezmoi_path "$rel")
    link_target=$(cat "$CHEZMOI_SOURCE/$rel")
    target_path="$HOME_DIR/$target"
    printf "    %s → %s (→ %s)\n" "$rel" "$target_path" "$link_target"
done

# Scripts: chezmoi run_onchange_after_* → scripts/ (extract from templates)
printf "\n  ${BOLD}Trigger scripts (→ scripts/):${RESET}\n"
for rel in "${SCRIPT_FILES[@]}"; do
    basename_script=$(basename "$rel")
    # Remove .tmpl extension — scripts become plain scripts
    dest_name="${basename_script%.tmpl}"
    # Categorize by directory (darwin/, linux/, or shared)
    case "$rel" in
        *.chezmoiscripts/darwin/*) dest_dir="scripts/darwin" ;;
        *.chezmoiscripts/linux/*) dest_dir="scripts/linux" ;;
        *) dest_dir="scripts" ;;
    esac
    # Clean up name: strip run_onchange_after_ prefix, strip .sh
    clean_name=$(echo "$dest_name" | sed 's/^run_\(onchange_\)\?after_//' | sed 's/^run_after_//')
    printf "    %s → %s/%s\n" "$rel" "$dest_dir" "$clean_name"
done

printf "\n"

# ─── Phase 4: Generate fleet.toml ────────────────────────────────────────────
info "Phase 4: fleet.toml generation plan"

printf "  Will merge:\n"
printf "    - hemma/fleet.toml (machine definitions)\n"
printf "    - .chezmoi.toml.tmpl (age recipients, per-machine data)\n"
printf "  Into: ~/dotfiles/fleet.toml (nit + hemma shared config)\n\n"

# ─── Phase 5: Generate triggers.toml ─────────────────────────────────────────
info "Phase 5: triggers.toml generation plan"

printf "  Will convert chezmoi run_onchange_after_* naming conventions:\n"
printf "    OS prefix (darwin/, linux/, shared) → os filter\n"
printf "    Hash comments in templates → watch globs\n"
printf "    is_dev/is_router conditionals → role filter\n\n"

printf "  Planned triggers.toml entries:\n"
for rel in "${SCRIPT_FILES[@]}"; do
    basename_script=$(basename "$rel" .tmpl)
    case "$rel" in
        *.chezmoiscripts/darwin/*) os_filter='os = "darwin"' ;;
        *.chezmoiscripts/linux/*) os_filter='os = "linux"' ;;
        *) os_filter="" ;;
    esac
    clean_name=$(echo "$basename_script" | sed 's/^run_\(onchange_\)\?after_//' | sed 's/^run_after_//' | sed 's/\.sh$//')
    printf "    [[trigger]] name = \"%s\" %s\n" "$clean_name" "$os_filter"
done
printf "\n"

# ─── Phase 6: Generate ~/.gitignore ──────────────────────────────────────────
info "Phase 6: ~/.gitignore (blacklist strategy)"

printf "  Will create ~/.gitignore with:\n"
printf "    /* (ignore all top-level non-dot items)\n"
printf "    !dotfiles/ (whitelist project hub)\n"
printf "    .cache/, .cargo/, .rustup/ (ignore large dotdirs)\n"
printf "    New dotfiles show up as untracked ✓\n\n"

# ─── Phase 7: Initialize bare repo ───────────────────────────────────────────
info "Phase 7: Bare repo initialization"

if [ -d "$NIT_REPO" ]; then
    warn "bare repo already exists at $NIT_REPO"
else
    if $DRY_RUN; then
        dry "git init --bare $NIT_REPO"
        dry "git --git-dir=$NIT_REPO --work-tree=$HOME add ..."
    fi
fi

# ─── Phase 8: Generate local.toml ────────────────────────────────────────────
info "Phase 8: local.toml generation"

hostname_val=$(hostname -s 2>/dev/null || echo "unknown")

# Try to match hostname against fleet.toml machine names
machine_name=""
fleet_toml="$DOTFILES_DIR/fleet.toml"
if [ -f "$fleet_toml" ] && command -v python3 >/dev/null 2>&1; then
    # Auto-detect from fleet.toml
    machine_name=$(python3 -c "
import tomllib, sys
from pathlib import Path
try:
    data = tomllib.loads(Path('$fleet_toml').read_text())
    hostname = '$hostname_val'.lower()
    for name, m in data.get('machines', {}).items():
        if name.lower() == hostname or hostname in name.lower() or name.lower() in hostname:
            print(name)
            sys.exit(0)
except: pass
" 2>/dev/null || true)
fi

# Fall back to hostname
if [ -z "$machine_name" ]; then
    machine_name="$hostname_val"
    warn "could not match hostname to fleet.toml — using '$machine_name'"
    warn "edit ~/.config/nit/local.toml to set the correct machine name"
fi

printf "  Machine: %s (from hostname: %s)\n" "$machine_name" "$hostname_val"
printf "  Will write: ~/.config/nit/local.toml\n\n"

# ─── Summary ──────────────────────────────────────────────────────────────────

printf "\n${BOLD}Migration summary:${RESET}\n"
printf "  %d plain files → \$HOME (bare git tracked)\n" "$plain_count"
printf "  %d templates → ~/dotfiles/templates/\n" "$template_count"
printf "  %d secrets → ~/dotfiles/secrets/\n" "$secret_count"
printf "  %d trigger scripts → ~/dotfiles/scripts/\n" "$script_count"
printf "  %d symlinks → real symlinks\n" "$symlink_count"
printf "  + fleet.toml (merged from hemma + chezmoi)\n"
printf "  + triggers.toml (from run_onchange naming)\n"
printf "  + ~/.gitignore (blacklist strategy)\n"
printf "  + ~/.config/nit/local.toml\n"
printf "  + bare repo at %s\n" "$NIT_REPO"

if $DRY_RUN; then
    printf "\n${BOLD}${YELLOW}DRY RUN COMPLETE${RESET} — no files were modified.\n"
    printf "Run with ${BOLD}--execute${RESET} to perform the migration.\n"
    printf "The master branch stays untouched as rollback.\n"
else
    printf "\n${BOLD}${GREEN}Migration plan ready.${RESET}\n"
    printf "TODO: Execute the actual file movements (Phase 3-8 execution not yet implemented).\n"
    printf "This script currently plans the migration. Execution will be added in T-14.\n"
fi
