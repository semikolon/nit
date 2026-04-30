# CC Session Audit — nit, last 3 months (2026-04-29)

**Status**: TENTATIVE — needs human review before any action.
**Scope**: nit has NO dedicated CC session dir (project work happens from dotfiles sessions). Searched dotfiles sessions for nit-specific work since 2026-01-29.
**Cross-references**: `~/dotfiles/docs/cc_session_audit_master_2026_04_29.md` (cross-project synthesis); `~/Projects/nit/docs/MIGRATION_CHECKLIST.md` (operational); `~/dotfiles/.claude/specs/nit/{requirements,design,tasks}.md` (canonical spec).

## Hanging sessions

Work continued in dotfiles sessions, never restarted in a nit-specific session:
- **`7b91e84d`** (Apr 21, 22:45) — Tier 0 dedup commits via `nit add`/`nit commit`; surfaced ergonomics gripes mid-flow
- **`d10658c7`** (Apr 21, 22:20) — The 17.8 MB nit migration session; v2 features designed but deferred
- **`5f9aaa7f`** (Apr 28, 09:47) — Routine dotfiles work using `nit`; minor friction noted
- **`dc76b99c`** (Apr 29, today) — Today's session; nothing new on nit itself

## Orphaned ideas (not in `nit/docs/MIGRATION_CHECKLIST.md`, dotfiles `TODO.md`, or `dotfiles/.claude/specs/nit/`)

1. **`nit apply` rename-BTM noise after reboots** (`7b91e84d`, verbatim user line): "expected first-bootstrap per agent per reboot. Can't silence — macOS design. Documented in this reply; not worth tracking in TODO." Acknowledged but not even captured as a "won't fix / known macOS quirk" doc note — first-time-after-reboot users will hit it again with no breadcrumb.

2. **Cross-session voicings store as nit-precedent pattern** (`7b91e84d`, Apr 21) — User quote: *"Post-nit, long-horizon: shared cross-session voicings store (Redis on Darwin — you already have `project-registry:v1` as precedent; same pattern, different key)."* TTS-roadmap plan doc has this; nit's own MIGRATION_CHECKLIST does NOT note the pattern as reusable infra for future fleet-shared state.

3. **Audit-any-other-too-broad-gitignore-rules-from-after-nit-migration** (`7b91e84d`, user request, only partially executed). The 3-wave audit completed in commits `db6b060`, `5b0c9ec`, `77ff343`/correction — but the systemic question "what's the test we run to catch this CLASS of bug?" was raised mid-flow and never turned into a regression test or pre-commit guard.

4. **`nit pick --apply` + `nit pick --apply --with-llm`** (designed in `d10658c7` / MIGRATION_CHECKLIST § "v2: Drift auto-promotion to source") — flagged "deferred v2" but user repeatedly hand-promoted drift this session; usage data from those sessions to inform the design was never captured.

5. **`local.toml` machine-name auto-derivation gotcha** (`baaf1a5` fix shipped) — but the deeper proposal "should `nit bootstrap` validate fleet.toml symmetry across machines (catch missing recipients before rekey)?" surfaced in `d10658c7` and was not added to any spec/checklist.

## Notes / patterns

Every substantial nit decision DID get persisted (the MIGRATION_CHECKLIST is exemplary). What slips through is **in-session friction notes** ("known macOS quirk", "we should regression-test that class") that would help the next CC session debug similar issues without having to re-derive.

These are *meta-friction* observations rather than novel ideas — but they're still recovery-worthy because future-Claude won't have access to the in-session reasoning that generated them, only the tool-output evidence.

## Recommended actions for nit

**Tier 2:**
- [ ] Add a "Known quirks" section to `nit/docs/MIGRATION_CHECKLIST.md` capturing item 1 (rename-BTM noise after reboots) + any other "won't fix / macOS design" observations from session JSONLs.
- [ ] Add item 2 (cross-session voicings store as nit-precedent pattern) to MIGRATION_CHECKLIST § "v2 considerations" — explicit note that nit's project-registry precedent generalizes to other fleet-shared state.
- [ ] `ccresume 7b91e84d` — turn the "what's the test for this class of bug?" question (item 3) into either a regression test in the nit test suite or a pre-commit guard in `~/.git-templates/hooks/pre-commit`.
- [ ] When designing `nit pick --apply` + `--with-llm` (v2), `ccresume d10658c7` to surface the usage data + design reasoning before re-deriving.
- [ ] Add a "fleet.toml symmetry validator" task to MIGRATION_CHECKLIST or `dotfiles/.claude/specs/nit/tasks.md` (item 5).

**Project status note**: nit is the most-disciplined project in this audit by orphan-density per session. The MIGRATION_CHECKLIST captures decision rationale exemplarily; the only orphan class is meta-friction observations that don't fit cleanly into existing doc structure.

**Resume commands:**
```
ccresume 7b91e84d   # nit ergonomics gripes + regression-test class question
ccresume d10658c7   # nit migration / v2 design seeds
```
