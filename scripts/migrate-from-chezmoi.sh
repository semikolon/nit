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
CHEZMOI_TOML_TMPL="$CHEZMOI_SOURCE/.chezmoi.toml.tmpl"
HEMMA_FLEET="$DOTFILES_DIR/hemma/fleet.toml"
NIT_TEMPLATES="$DOTFILES_DIR/templates"
NIT_SECRETS="$DOTFILES_DIR/secrets"
NIT_SCRIPTS="$DOTFILES_DIR/scripts"
NIT_REPO="$HOME/.local/share/nit/repo.git"
NIT_CONFIG_DIR="$HOME/.config/nit"
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

# Strip .tmpl extension from a resolved path (for template target mapping)
strip_tmpl() {
    local path="$1"
    echo "${path%.tmpl}"
}

# Test a few resolutions
printf "  Prefix resolution examples:\n"
for test_path in "dot_zshrc" "private_dot_secrets/encrypted_private_tier-all.env.age" "dot_claude/hooks/symlink_stop.sh" "private_Library/LaunchAgents/com.example.daemon.plist.tmpl" "dot_local/bin/executable_my-tool"; do
    resolved=$(resolve_chezmoi_path "$test_path")
    printf "    %s → %s\n" "$test_path" "$resolved"
done
printf "\n"

# ─── Go → Tera template conversion ──────────────────────────────────────────
# Converts chezmoi Go template syntax to Tera/Jinja2 syntax.
# Called when moving .tmpl files to templates/.
# Uses Python for reliable regex handling (no shell escaping issues with perl).
convert_go_to_tera() {
    local input_file="$1"

    CONVERT_INPUT="$input_file" python3 << 'PYEOF'
import re, sys, os
from pathlib import Path

content = Path(os.environ['CONVERT_INPUT']).read_text()

# Phase 1: Protect escaped Go braces (docker format strings)
# In chezmoi, {{`{{.Names}}`}} produces literal {{.Names}} in output.
# Convert to Tera raw blocks.
content = re.sub(
    r'\{\{\s*`([^`]*)`\s*\}\}',
    r'{% raw %}\1{% endraw %}',
    content
)

# Phase 2: Remove hash trigger lines (handled by triggers.toml in nit)
# Lines like: # hash: {{ include "dot_Brewfile" | sha256sum }}
# Also: # narrate-client src: {{ include "..." | sha256sum }}
content = re.sub(r'^[^\n]*\{\{.*?include.*?sha256sum.*?\}\}[^\n]*\n?', '', content, flags=re.MULTILINE)

# Phase 3: Remove version trigger lines used as chezmoi hash triggers
# Lines like: # version: v1.0.0  or  # statusline-version: v1.2.8-patched
content = re.sub(r'^#\s*(?:version|[a-z]+-version):\s*\S+[^\n]*\n?', '', content, flags=re.MULTILINE)

# Phase 4: Convert complex conditionals (or/and with multiple conditions)

def convert_or_conditional(m):
    full = m.group(0)
    inner = m.group(1)
    ws_start = "{%-" if full.startswith("{{-") else "{%"
    ws_end = "-%}" if full.endswith("-%}}") or full.endswith("-}}") else "%}"

    conditions = []
    for eq_m in re.finditer(r'\(eq\s+\.(?:chezmoi\.)?(\w+)\s+"([^"]+)"\)', inner):
        var, val = eq_m.group(1, 2)
        conditions.append(f'{var} == "{val}"')

    if conditions:
        joined = " or ".join(conditions)
        return f"{ws_start} if {joined} {ws_end}"
    return full

content = re.sub(
    r'\{\{-?\s*if\s+or\s+(.*?)\s*-?\}\}',
    convert_or_conditional,
    content
)

def convert_and_conditional(m):
    full = m.group(0)
    inner = m.group(1)
    ws_start = "{%-" if full.startswith("{{-") else "{%"
    ws_end = "-%}" if full.endswith("-%}}") or full.endswith("-}}") else "%}"

    conditions = []
    for eq_m in re.finditer(r'\(eq\s+\.(?:chezmoi\.)?(\w+)\s+"([^"]+)"\)', inner):
        var, val = eq_m.group(1, 2)
        conditions.append(f'{var} == "{val}"')
    for ne_m in re.finditer(r'\(ne\s+\.(?:chezmoi\.)?(\w+)\s+"([^"]+)"\)', inner):
        var, val = ne_m.group(1, 2)
        conditions.append(f'{var} != "{val}"')
    for not_m in re.finditer(r'\(not\s+\.(\w+)\)', inner):
        conditions.append(f'not {not_m.group(1)}')
    # Bare boolean vars (not inside parens) — e.g., .is_router
    for bare_m in re.finditer(r'(?<!\w)\.(\w+)(?!\w)(?!\s*")', inner):
        var = bare_m.group(1)
        # Skip if already captured as part of eq/ne/not
        if var not in ('chezmoi',) and f'{var} ==' not in ' '.join(conditions) and f'{var} !=' not in ' '.join(conditions) and f'not {var}' not in conditions:
            conditions.append(var)

    if conditions:
        joined = " and ".join(conditions)
        return f"{ws_start} if {joined} {ws_end}"
    return full

content = re.sub(
    r'\{\{-?\s*if\s+and\s+(.*?)\s*-?\}\}',
    convert_and_conditional,
    content
)

# Phase 5: Convert simple conditionals

def detect_ws(m):
    """Detect whitespace trimming from original Go delimiters."""
    full = m.group(0)
    ws_start = "{%-" if re.match(r'\{\{-', full) else "{%"
    ws_end = "-%}" if re.search(r'-\}\}$', full) else "%}"
    return ws_start, ws_end

# if eq .chezmoi.X "Y" → if X == "Y"
def convert_if_eq_chezmoi(m):
    ws_start, ws_end = detect_ws(m)
    var = m.group(1)
    val = m.group(2)
    return f'{ws_start} if {var} == "{val}" {ws_end}'

content = re.sub(
    r'\{\{-?\s*if\s+eq\s+\.chezmoi\.(\w+)\s+"([^"]+)"\s*-?\}\}',
    convert_if_eq_chezmoi,
    content
)

# if ne .chezmoi.X "Y" → if X != "Y"
def convert_if_ne_chezmoi(m):
    ws_start, ws_end = detect_ws(m)
    var = m.group(1)
    val = m.group(2)
    return f'{ws_start} if {var} != "{val}" {ws_end}'

content = re.sub(
    r'\{\{-?\s*if\s+ne\s+\.chezmoi\.(\w+)\s+"([^"]+)"\s*-?\}\}',
    convert_if_ne_chezmoi,
    content
)

# if eq .X "Y" (custom data like .chezmoi.arch handled above, this catches remaining)
def convert_if_eq_data(m):
    ws_start, ws_end = detect_ws(m)
    var = m.group(1)
    val = m.group(2)
    return f'{ws_start} if {var} == "{val}" {ws_end}'

content = re.sub(
    r'\{\{-?\s*if\s+eq\s+\.(\w+)\s+"([^"]+)"\s*-?\}\}',
    convert_if_eq_data,
    content
)

# if not .var → if not var
def convert_if_not(m):
    ws_start, ws_end = detect_ws(m)
    var = m.group(1)
    return f'{ws_start} if not {var} {ws_end}'

content = re.sub(
    r'\{\{-?\s*if\s+not\s+\.(\w+)\s*-?\}\}',
    convert_if_not,
    content
)

# if .var → if var (bare boolean)
def convert_if_bare(m):
    ws_start, ws_end = detect_ws(m)
    var = m.group(1)
    return f'{ws_start} if {var} {ws_end}'

content = re.sub(
    r'\{\{-?\s*if\s+\.(\w+)\s*-?\}\}',
    convert_if_bare,
    content
)

# else
def convert_else(m):
    ws_start, ws_end = detect_ws(m)
    return f'{ws_start} else {ws_end}'

content = re.sub(r'\{\{-?\s*else\s*-?\}\}', convert_else, content)

# end → endif
def convert_end(m):
    ws_start, ws_end = detect_ws(m)
    return f'{ws_start} endif {ws_end}'

content = re.sub(r'\{\{-?\s*end\s*-?\}\}', convert_end, content)

# Phase 6: Convert variable references
content = re.sub(r'\{\{-?\s*\.chezmoi\.homeDir\s*-?\}\}', '{{ home_dir }}', content)
content = re.sub(r'\{\{-?\s*\.chezmoi\.hostname\s*-?\}\}', '{{ hostname }}', content)
content = re.sub(r'\{\{-?\s*\.chezmoi\.os\s*-?\}\}', '{{ os }}', content)
content = re.sub(r'\{\{-?\s*\.chezmoi\.arch\s*-?\}\}', '{{ arch }}', content)
# Generic .var_name → {{ var_name }}
content = re.sub(r'\{\{-?\s*\.([a-zA-Z_]\w*)\s*-?\}\}', r'{{ \1 }}', content)

# Phase 7: Convert joinPath expressions
# {{ joinPath .chezmoi.homeDir "path" | quote }} → "{{ home_dir }}/path"
content = re.sub(
    r'\{\{\s*joinPath\s+\.chezmoi\.homeDir\s+"([^"]+)"\s*\|\s*quote\s*\}\}',
    r'"{{ home_dir }}/\1"',
    content
)

# Phase 8: Convert merge command template escapes
# chezmoi uses {{ "{{" }} and {{ "}}" }} to produce literal {{ and }}
content = re.sub(r'\{\{\s*"(\{\{)"\s*\}\}', r'{% raw %}\1{% endraw %}', content)
content = re.sub(r'\{\{\s*"(\}\})"\s*\}\}', r'{% raw %}\1{% endraw %}', content)

sys.stdout.write(content)
PYEOF
}

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
    clean_name=$(echo "$dest_name" | sed -E 's/^run_(onchange_)?after_//' | sed 's/^run_after_//')
    printf "    %s → %s/%s\n" "$rel" "$dest_dir" "$clean_name"
done

printf "\n"

# ─── Phase 3 execute: Move files ─────────────────────────────────────────────
if ! $DRY_RUN; then
    info "Phase 3 execute: Moving files"

    # Create nit directories
    mkdir -p "$NIT_TEMPLATES" "$NIT_SECRETS"

    # --- Templates: move .tmpl files to templates/, convert Go→Tera ---
    tmpl_moved=0
    for rel in "${TEMPLATE_FILES[@]}"; do
        target=$(resolve_chezmoi_path "$rel")
        dest="$NIT_TEMPLATES/$target"
        dest_dir_path=$(dirname "$dest")
        mkdir -p "$dest_dir_path"

        # Read source, convert Go→Tera, write to templates/
        src_content=$(cat "$CHEZMOI_SOURCE/$rel")
        converted=$(convert_go_to_tera "$src_content")
        printf '%s\n' "$converted" > "$dest"

        tmpl_moved=$((tmpl_moved + 1))
    done
    ok "moved $tmpl_moved templates to $NIT_TEMPLATES/ (Go→Tera converted)"

    # --- Secrets: move .age files to secrets/ ---
    secret_moved=0
    for rel in "${SECRET_FILES[@]}"; do
        target=$(resolve_chezmoi_path "$rel")
        tier_name=$(echo "$target" | sed 's/.*\///' | sed 's/\.env\.age$//')
        dest="$NIT_SECRETS/${tier_name}.env.age"
        cp "$CHEZMOI_SOURCE/$rel" "$dest"
        secret_moved=$((secret_moved + 1))
    done
    ok "moved $secret_moved secrets to $NIT_SECRETS/"

    # --- Scripts: move trigger scripts to scripts/ ---
    script_moved=0
    for rel in "${SCRIPT_FILES[@]}"; do
        basename_script=$(basename "$rel")
        dest_name="${basename_script%.tmpl}"
        case "$rel" in
            *.chezmoiscripts/darwin/*) dest_dir="$DOTFILES_DIR/scripts/darwin" ;;
            *.chezmoiscripts/linux/*) dest_dir="$DOTFILES_DIR/scripts/linux" ;;
            *) dest_dir="$DOTFILES_DIR/scripts" ;;
        esac
        mkdir -p "$dest_dir"

        clean_name=$(echo "$dest_name" | sed -E 's/^run_(onchange_)?after_//' | sed 's/^run_after_//')
        dest_path="$dest_dir/$clean_name"

        # If the script is a .tmpl, convert Go→Tera, then strip template wrapper
        # (scripts become plain scripts — conditionals become comments/removed)
        if [[ "$basename_script" == *.tmpl ]]; then
            src_content=$(cat "$CHEZMOI_SOURCE/$rel")
            # Convert Go→Tera for any remaining template syntax
            converted=$(convert_go_to_tera "$src_content")
            printf '%s\n' "$converted" > "$dest_path"
        else
            cp "$CHEZMOI_SOURCE/$rel" "$dest_path"
        fi
        chmod +x "$dest_path"
        script_moved=$((script_moved + 1))
    done
    ok "moved $script_moved trigger scripts to $DOTFILES_DIR/scripts/"

    # --- Symlinks: convert symlink_ files to real symlinks ---
    symlink_created=0
    for rel in "${SYMLINK_FILES[@]}"; do
        target=$(resolve_chezmoi_path "$rel")
        link_target=$(cat "$CHEZMOI_SOURCE/$rel")
        target_path="$HOME_DIR/$target"
        target_dir=$(dirname "$target_path")
        mkdir -p "$target_dir"

        # Remove existing file/symlink if present, create real symlink
        if [ -e "$target_path" ] || [ -L "$target_path" ]; then
            rm -f "$target_path"
        fi
        ln -s "$link_target" "$target_path"
        symlink_created=$((symlink_created + 1))
    done
    ok "created $symlink_created real symlinks"

    # --- Plain files: already at target (chezmoi deployed them) ---
    # Nothing to physically move. The bare repo will track the target paths.
    ok "$plain_count plain files already at target paths (chezmoi deployed)"
fi

# ─── Phase 4: Generate fleet.toml ────────────────────────────────────────────
info "Phase 4: fleet.toml generation plan"

printf "  Will merge:\n"
printf "    - hemma/fleet.toml (machine definitions)\n"
printf "    - .chezmoi.toml.tmpl (age recipients, per-machine data)\n"
printf "  Into: ~/dotfiles/fleet.toml (nit + hemma shared config)\n\n"

# ─── Phase 4 execute: Generate fleet.toml ─────────────────────────────────────
if ! $DRY_RUN; then
    info "Phase 4 execute: Generating fleet.toml"

    fleet_out="$DOTFILES_DIR/fleet.toml"

    # Extract age recipients from .chezmoi.toml.tmpl
    age_recipients=""
    if [ -f "$CHEZMOI_TOML_TMPL" ]; then
        # Read lines between recipients = [ and ]
        age_recipients=$(sed -n '/recipients = \[/,/\]/p' "$CHEZMOI_TOML_TMPL" | grep '"age1' | sed 's/.*"\(age1[^"]*\)".*/\1/')
    fi

    # Extract machine data from hemma/fleet.toml
    # Parse hemma fleet.toml and .chezmoi.toml.tmpl to build nit fleet.toml
    python3 -c "
import sys, re
from pathlib import Path

hemma_path = '$HEMMA_FLEET'
chezmoi_path = '$CHEZMOI_TOML_TMPL'
output_path = '$fleet_out'

# --- Parse hemma fleet.toml ---
hemma_content = Path(hemma_path).read_text()
machines = {}
# Simple TOML parser for [machines.X] sections
current_machine = None
for line in hemma_content.splitlines():
    m = re.match(r'\[machines\.(\w+)\]', line)
    if m:
        current_machine = m.group(1)
        machines[current_machine] = {}
        continue
    if current_machine and '=' in line and not line.strip().startswith('#'):
        key, val = line.split('=', 1)
        key = key.strip()
        val = val.strip()
        # Strip inline comments
        if '#' in val:
            val = val[:val.index('#')].strip()
        machines[current_machine][key] = val

# --- Parse chezmoi data for role mapping ---
chezmoi_content = Path(chezmoi_path).read_text()

# Extract is_dev hostnames
dev_hosts = set()
router_hosts = set()
for m in re.finditer(r'eq \.chezmoi\.hostname \"([^\"]+)\"', chezmoi_content):
    hostname = m.group(1)
    # Check context: is it in the is_dev block or is_router block?
# More precise: find the is_dev and is_router blocks
dev_block = re.search(r'is_dev = true.*?end', chezmoi_content, re.DOTALL)
router_block = re.search(r'is_router = true.*?end', chezmoi_content, re.DOTALL)

if dev_block:
    for m in re.finditer(r'eq \.chezmoi\.hostname \"([^\"]+)\"', dev_block.group()):
        dev_hosts.add(m.group(1))
if router_block:
    for m in re.finditer(r'eq \.chezmoi\.hostname \"([^\"]+)\"', router_block.group()):
        router_hosts.add(m.group(1))

# Extract age recipients
age_lines = []
in_recipients = False
for line in chezmoi_content.splitlines():
    if 'recipients = [' in line:
        in_recipients = True
        continue
    if in_recipients:
        if ']' in line:
            break
        m = re.search(r'\"(age1[^\"]+)\"', line)
        if m:
            age_lines.append(m.group(1))

# --- Map hemma roles to nit array format ---
def build_roles(machine_name, hemma_role_str):
    roles = []
    hemma_roles = [r.strip().strip('\"') for r in hemma_role_str.split(',')]
    for r in hemma_roles:
        roles.append(r)
    # Add 'dev' if machine is in dev_hosts (by matching machine name variants)
    # Check against chezmoi hostnames
    name_lower = machine_name.lower()
    for h in dev_hosts:
        h_lower = h.lower()
        if name_lower in h_lower or h_lower in name_lower or name_lower == h_lower:
            if 'dev' not in roles:
                roles.append('dev')
            break
    return roles

# --- Build output ---
out = []
out.append('# nit fleet inventory')
out.append('# Merged from hemma/fleet.toml + .chezmoi.toml.tmpl')
out.append('# Machine definitions shared by nit and hemma')
out.append('')

# Add mac-mini (localhost, not in hemma fleet since it runs locally)
out.append('[machines.mac-mini]')
out.append('ssh_host = \"localhost\"')
out.append('role = [\"dev\", \"primary\"]')
out.append('')

for name, data in machines.items():
    out.append(f'[machines.{name}]')
    ssh_host = data.get('ssh_host', '\"' + name + '\"')
    out.append(f'ssh_host = {ssh_host}')
    hemma_role = data.get('role', '\"\"').strip('\"')
    roles = build_roles(name, hemma_role)
    role_str = ', '.join(f'\"{r}\"' for r in roles)
    out.append(f'role = [{role_str}]')
    if data.get('critical', '').strip() == 'true':
        out.append('critical = true')
    out.append('')

# Templates section
out.append('# Template configuration')
out.append('[templates]')
out.append('source_dir = \"~/dotfiles/templates\"')
out.append('')

# Secrets section with tier definitions
out.append('# Encryption — age recipients per tier')
out.append('[secrets]')
out.append('source_dir = \"~/dotfiles/secrets\"')
out.append('')

# Build tier-to-recipient mapping based on chezmoi's four-tier model
# All recipients go in tier-all
out.append('[secrets.tiers.tier-all]')
recip_str = ', '.join(f'\"{r}\"' for r in age_lines)
out.append(f'recipients = [{recip_str}]')
out.append('target = \"~/.secrets/tier-all.env\"')
out.append('')

# tier-servers: mac-mini + darwin (first 2 recipients typically)
# We preserve all recipients and let the user refine
out.append('[secrets.tiers.tier-servers]')
out.append(f'recipients = [{recip_str}]')
out.append('target = \"~/.secrets/tier-servers.env\"')
out.append('# TODO: Narrow to mac-mini + darwin recipients only')
out.append('')

out.append('[secrets.tiers.tier-mac]')
out.append(f'recipients = [{recip_str}]')
out.append('target = \"~/.secrets/tier-mac.env\"')
out.append('# TODO: Narrow to mac-mini + mbp recipients only')
out.append('')

out.append('[secrets.tiers.tier-edge]')
out.append(f'recipients = [{recip_str}]')
out.append('target = \"~/.secrets/tier-edge.env\"')
out.append('# TODO: Narrow to mac-mini + shannon recipients only')
out.append('')

# Permissions
out.append('# File permissions (non-default)')
out.append('[permissions]')
out.append('private = [\"~/.ssh/*\", \"~/.secrets/*\", \"~/.config/nit/age-key.txt\"]')
out.append('')

# Sync config
out.append('# Nightly sync')
out.append('[sync]')
out.append('command = \"nit update\"')
out.append('schedule = \"03:00\"')
out.append('idle_gated = true')

Path(output_path).write_text('\n'.join(out) + '\n')
" 2>&1

    if [ -f "$fleet_out" ]; then
        ok "generated $fleet_out"
    else
        err "failed to generate fleet.toml"
    fi
fi

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
    clean_name=$(echo "$basename_script" | sed -E 's/^run_(onchange_)?after_//' | sed 's/^run_after_//' | sed 's/\.sh$//')
    printf "    [[trigger]] name = \"%s\" %s\n" "$clean_name" "$os_filter"
done
printf "\n"

# ─── Phase 5 execute: Generate triggers.toml ─────────────────────────────────
if ! $DRY_RUN; then
    info "Phase 5 execute: Generating triggers.toml"

    triggers_out="$DOTFILES_DIR/triggers.toml"

    # Build triggers.toml by analyzing each script file
    python3 -c "
import re, sys
from pathlib import Path

chezmoi_source = '$CHEZMOI_SOURCE'
dotfiles_dir = '$DOTFILES_DIR'
output_path = '$triggers_out'

script_files = '''$(printf '%s\n' "${SCRIPT_FILES[@]}")'''.strip().splitlines()

out = []
out.append('# nit trigger definitions')
out.append('# Converted from chezmoi run_onchange_after_* scripts')
out.append('# Watch globs extracted from hash comments in script templates')
out.append('')

for rel in script_files:
    if not rel.strip():
        continue

    basename_script = Path(rel).name
    # Strip .tmpl extension
    if basename_script.endswith('.tmpl'):
        basename_script = basename_script[:-5]

    # Determine OS filter from directory
    os_filter = None
    if '/.chezmoiscripts/darwin/' in f'/{rel}' or rel.startswith('.chezmoiscripts/darwin/'):
        os_filter = 'darwin'
    elif '/.chezmoiscripts/linux/' in f'/{rel}' or rel.startswith('.chezmoiscripts/linux/'):
        os_filter = 'linux'

    # Clean name: strip run_onchange_after_ or run_after_ prefix and .sh suffix
    clean = re.sub(r'^run_(onchange_)?after_', '', basename_script)
    clean = re.sub(r'^run_after_', '', clean)
    clean = re.sub(r'\.sh$', '', clean)

    # Determine script dest path
    if os_filter == 'darwin':
        script_path = f'scripts/darwin/{clean}.sh'
    elif os_filter == 'linux':
        script_path = f'scripts/linux/{clean}.sh'
    else:
        script_path = f'scripts/{clean}.sh'

    # Read script content to extract watch globs and role filters
    src_path = Path(chezmoi_source) / rel
    content = ''
    if src_path.exists():
        content = src_path.read_text()

    # Extract watch globs from hash comments
    # Patterns: # hash: {{ include \"path\" | sha256sum }}
    # Also: # thing src: {{ include \"path\" | sha256sum }}
    watch_globs = []
    for m in re.finditer(r'include\s+\"([^\"]+)\"', content):
        chezmoi_path = m.group(1)
        # Convert chezmoi source path to target path
        # Strip dot_ → ., private_dot_ → ., private_ → strip, executable_ → strip
        parts = chezmoi_path.split('/')
        converted_parts = []
        for part in parts:
            resolved = part
            if resolved.startswith('encrypted_private_dot_'):
                resolved = '.' + resolved[len('encrypted_private_dot_'):]
            elif resolved.startswith('encrypted_private_'):
                resolved = resolved[len('encrypted_private_'):]
            elif resolved.startswith('private_dot_'):
                resolved = '.' + resolved[len('private_dot_'):]
            elif resolved.startswith('private_'):
                resolved = resolved[len('private_'):]
            elif resolved.startswith('dot_'):
                resolved = '.' + resolved[len('dot_'):]
            resolved = resolved.replace('executable_', '')
            # Strip .tmpl extension for target path
            if resolved.endswith('.tmpl'):
                resolved = resolved[:-5]
            converted_parts.append(resolved)
        target_glob = '/'.join(converted_parts)
        watch_globs.append(target_glob)

    # Detect source directory watches (glob patterns for multiple files)
    # e.g., .claude/hooks/*/Cargo.toml → watch with glob
    # For scripts that watch multiple source files in a pattern, use glob
    # Check if script watches Cargo.toml files (Rust hooks)
    if 'rust' in clean.lower() or 'build' in clean.lower():
        # Check for multiple include patterns referencing same directory structure
        cargo_dirs = set()
        for g in watch_globs:
            if 'Cargo.toml' in g or '/src/' in g:
                parts = g.split('/')
                for i, p in enumerate(parts):
                    if p in ('Cargo.toml', 'src'):
                        cargo_dirs.add('/'.join(parts[:i]))
                        break
        if cargo_dirs:
            # Replace individual file watches with glob patterns
            glob_watches = []
            seen_dirs = set()
            for d in cargo_dirs:
                if d not in seen_dirs:
                    glob_watches.append(f'{d}/Cargo.toml')
                    glob_watches.append(f'{d}/src/**')
                    seen_dirs.add(d)
            # Also keep non-Rust watches
            for g in watch_globs:
                if 'Cargo.toml' not in g and '/src/' not in g:
                    glob_watches.append(g)
            watch_globs = glob_watches

    # Deduplicate
    watch_globs = list(dict.fromkeys(watch_globs))

    # Extract role filter from conditionals
    role = None
    if re.search(r'if\s+not\s+\.is_dev', content):
        # Script exits early on non-dev — means it requires dev role
        role = 'dev'
    elif re.search(r'if.*\.is_router', content):
        role = 'router'
    # Check for hostname-specific conditionals at the top (template wrapper)
    hostname_filter = None
    top_conditional = re.match(r'\{\{-?\s*if\s+(.*?)\s*-?\}\}', content)
    if top_conditional:
        cond = top_conditional.group(1)
        if 'eq .chezmoi.os' in cond:
            # Already handled by os_filter from directory
            pass
        elif '.is_router' in cond and 'ne .chezmoi.os' in cond:
            role = 'router'
            # The ne darwin check is redundant since os=linux already
        elif '.is_dev' in cond:
            # Script gated on is_dev
            pass  # already detected above or from not .is_dev

    # Write trigger entry
    out.append('[[trigger]]')
    out.append(f'name = \"{clean}\"')
    out.append(f'script = \"{script_path}\"')
    if watch_globs:
        glob_str = ', '.join(f'\"{g}\"' for g in watch_globs)
        out.append(f'watch = [{glob_str}]')
    if os_filter:
        out.append(f'os = \"{os_filter}\"')
    if role:
        out.append(f'role = \"{role}\"')
    out.append('')

Path(output_path).write_text('\n'.join(out) + '\n')
" 2>&1

    if [ -f "$triggers_out" ]; then
        ok "generated $triggers_out"
    else
        err "failed to generate triggers.toml"
    fi
fi

# ─── Phase 6: Generate ~/.gitignore ──────────────────────────────────────────
info "Phase 6: ~/.gitignore (blacklist strategy)"

printf "  Will create ~/.gitignore with:\n"
printf "    /* (ignore all top-level non-dot items)\n"
printf "    !dotfiles/ (whitelist project hub)\n"
printf "    .cache/, .cargo/, .rustup/ (ignore large dotdirs)\n"
printf "    New dotfiles show up as untracked ✓\n\n"

# ─── Phase 6 execute: Create ~/.gitignore ─────────────────────────────────────
if ! $DRY_RUN; then
    info "Phase 6 execute: Creating ~/.gitignore"

    gitignore_path="$HOME_DIR/.gitignore"

    cat > "$gitignore_path" << 'GITIGNORE_EOF'
# nit blacklist strategy — ignore everything, whitelist dotfiles
# New dotfiles (.foorc) show up as untracked automatically.
# Non-dot top-level dirs (Projects/, Documents/) are ignored.

# ─── Ignore all top-level non-dot items ──────────────────────────
/*

# ─── Whitelist the project hub ───────────────────────────────────
!dotfiles/

# ─── Whitelist bin directory ─────────────────────────────────────
!bin/

# ─── Large dotdirs (caches, build artifacts, runtimes) ──────────
.cache/
.cargo/
.rustup/
.npm/
.nvm/
.local/pipx/
.local/share/mise/
.local/share/nit/
.local/share/chezmoi/
.conda/
.miniforge/
.docker/
.orbstack/
.Trash/
.volta/

# ─── Application state (not config) ─────────────────────────────
.zsh_history
.zsh_sessions/
.lesshst
.python_history
.irb_history
.node_repl_history
.viminfo
.wget-hsts

# ─── macOS noise ────────────────────────────────────────────────
.DS_Store
.CFUserTextEncoding
.Xauthority

# ─── IDE/editor state ───────────────────────────────────────────
.vscode/
.zed/

# ─── Large application data dirs ────────────────────────────────
.ollama/
.graphiti/
Library/
Movies/
Music/
Pictures/
GITIGNORE_EOF

    ok "created $gitignore_path"
fi

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

# ─── Phase 7 execute: Initialize bare repo ────────────────────────────────────
if ! $DRY_RUN; then
    if [ -d "$NIT_REPO" ]; then
        warn "bare repo already exists at $NIT_REPO — skipping init"
    else
        info "Phase 7 execute: Initializing bare repo"

        mkdir -p "$(dirname "$NIT_REPO")"
        git init --bare "$NIT_REPO"
        ok "created bare repo at $NIT_REPO"

        # Configure work tree
        git --git-dir="$NIT_REPO" --work-tree="$HOME_DIR" config core.worktree "$HOME_DIR"
        ok "configured work-tree = $HOME_DIR"

        # Set core.excludesFile to the home gitignore
        git --git-dir="$NIT_REPO" --work-tree="$HOME_DIR" config core.excludesFile "$HOME_DIR/.gitignore"
        ok "configured core.excludesFile = $HOME_DIR/.gitignore"

        # Don't show untracked files by default (too noisy with $HOME as worktree)
        git --git-dir="$NIT_REPO" --work-tree="$HOME_DIR" config status.showUntrackedFiles no
        ok "configured status.showUntrackedFiles = no"
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

# ─── Phase 8 execute: Generate local.toml ─────────────────────────────────────
if ! $DRY_RUN; then
    info "Phase 8 execute: Generating local.toml"

    mkdir -p "$NIT_CONFIG_DIR"

    cat > "$NIT_CONFIG_DIR/local.toml" << LOCALTOML_EOF
# Generated by nit migration script
# Identifies this machine to nit — looked up in fleet.toml
machine = "$machine_name"
identity = "~/.config/nit/age-key.txt"
LOCALTOML_EOF

    ok "generated $NIT_CONFIG_DIR/local.toml (machine = $machine_name)"
fi

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
    printf "\n${BOLD}${GREEN}MIGRATION COMPLETE${RESET}\n"
    printf "  Next steps:\n"
    printf "    1. Review generated files: fleet.toml, triggers.toml, templates/\n"
    printf "    2. Verify template conversion: diff templates/*.tmpl against originals\n"
    printf "    3. Refine fleet.toml tier recipients (marked with TODO comments)\n"
    printf "    4. Test: nit apply (once nit is built)\n"
    printf "    5. The master branch is preserved as rollback\n"
fi
