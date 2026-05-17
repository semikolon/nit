# nit — TODO

> nit's forward-work has historically been tracked in `docs/MIGRATION_CHECKLIST.md`
> (v2 drift-promotion to source, fleet rollout mechanism, etc.). This file is the
> project TODO. The entry below was logged here per explicit user request
> (2026-05-17) as a **self-contained, incident-driven task cluster** for a
> dedicated future CC session — it deliberately carries full incident context
> because the redesign tasks cannot be designed correctly without it.

---

## Concurrent-session commit safety + commit/deploy decoupling — incident-driven (2026-05-17)

**Status (2026-05-17, partially actioned):** committed `9b49e37`. Since then, two of these landed and the rest were *reframed by founding-spec archaeology* (see "Archaeology reframe" below — it materially changes the approach):
- ✅ **Commit-posture recalibration** — LANDED as the "Commit caution scales with reversibility" subsection in `~/.claude/CLAUDE.md` § Git Workflow Protocol (dotfiles commit `99b6503c`).
- ✅ **Forward-only-sync feature** — IMPLEMENTED + verified (nit commit `3d6bfec`, LOCAL, not pushed; 189 tests incl. 7 RED-GREEN + 2 SAFETY assertions; new code clippy-clean). Resolves the perpetual runtime-noise + nightly-sync-ABORT for the decisions-state/cache + spela-config files. Activation gates (USER): declare the 3 paths in `fleet.toml [sync] forward_only`, then `.nit-version` bump. (Was a deferred MIGRATION_CHECKLIST v2 item — now done; that entry trued.)
- ✅ **clippy `--all-targets` test-target hygiene — CLEARED** (this commit; separate scoped commit, local, not pushed; 189 tests green before+after). **Reconciliation correction to the prior framing:** these 11 were NOT a CI/push blocker. CI runs `cargo clippy -- -D warnings` (lib/bin targets only, no `--tests`) and was already GREEN — the lib/bin clippy errors had been cleared earlier in `f2c92ee`. The 11 remaining were `#[cfg(test)]`-only lints surfaced solely by `cargo clippy --all-targets` under Rust 1.93's stricter test-code linting. Cleared deliberately as a SEPARATE scoped commit (NOT bundled into the keystone — Co-triage / commit-caution). Sites: config.rs ×1 (needless borrow), encrypt.rs ×4 (`&[x.clone()]`→`std::slice::from_ref`), syncbase.rs ×4 (nested-if→let-chains), trigger.rs ×2 (useless `vec!`, field-reassign-with-default).
- ⏭️ **Recommended next:** the remaining commit-safety keystone + nit-transparency fix are **safety-critical `cmd_commit` surgery** — do them in a FRESH FOCUSED session with full RED-GREEN, NOT at the tail of a long session (per *Sequence by warmth-leverage*'s "warmth-becomes-liability" clause). The archaeology reframe below is the authority for the spec-conformance framing.

**Why this is intentionally detailed (not a TODO.md-discipline violation):** the Tier-2 redesign of `nit commit` cannot be done correctly without understanding the concurrent-session + template-mode + commit/deploy-coupling failure. The Context and Observed-behaviors sections below are *load-bearing rationale for the redesign*, not chronicle-for-its-own-sake. User explicitly requested "exhaustive details about what led up to this."

### Context — what led to this

Two **concurrent CC sessions** were running on the Mac Mini, both operating on the *single shared* nit bare repo (`~/.local/share/nit/repo.git`) + `$HOME` work-tree + git index, with **zero awareness of each other** (no agent-coordination layer installed — `mcp-agent-mail`/`am` were deliberately deferred per dotfiles CLAUDE.md "until explicit need surfaces"; this incident is plausibly that signal).

- **Session 1 (Shannon Demeter-rootfs fix):** edited 3 dotfiles docs (`dotfiles/CLAUDE.md`, `dotfiles/TODO.md`, `dotfiles/docs/demeter_excavation_2026_05_14.md`) plus a system-overlay file (`dotfiles/system/shannon/usr/local/bin/shannon-userspace-watchdog`).
- **Session 2 (skill-loading "pushy-description" sweep):** edited 17 `~/.claude/skills/*/SKILL.md` + `dotfiles/TODO.md` + `dotfiles/templates/.zshenv.tmpl` (added `SLASH_COMMAND_TOOL_CHAR_BUDGET=30000`). User asked Session 2 to "commit and push."

**Timeline of the collision:**

1. Session 1 `nit add`-ed 4 explicit Shannon paths → committed `b643880c` **cleanly, correctly scoped to exactly 4 files** ("nit: committed (plain files only, no templates)"). **This is the correct-behavior reference point**: when *no template is drifted*, `nit commit` honors the intended plain-file scope.
2. Session 1 later re-edited the 3 Shannon docs (truing to RESOLVED), `nit add`-ed exactly those 3 by explicit path, and verified the staged set was exactly those 3.
3. Session 1 ran `nit commit`. It **BLOCKED** on the session-anchor ack-gate, citing `.zshenv.tmpl` — a **template with drift created by Session 2, not Session 1**. Message: *"first commit attempt — ack written, re-run nit commit to proceed."*
4. Session 1 treated the ack-block as ceremony-to-clear and **re-ran `nit commit`**. The post-ack commit went down the **template-mode** path and:
   - (a) committed the **ENTIRE 21-file modified set** (Session 1's 3 docs + Session 2's 17 skills + `.zshenv.tmpl`) into `3cf94eb8`, **silently overriding Session 1's explicit `nit add` scoping** (`git commit -a`-like); and
   - (b) **rendered + deployed 17 templates to live targets** on the Mac Mini, including writing `~/.zshenv` with Session 2's in-progress `SLASH_COMMAND_TOOL_CHAR_BUDGET=30000`.
5. Independently, Session 2 attempted its own "commit and push", found Session 1's `dotfiles/CLAUDE.md` + `demeter_excavation` **already staged in the shared index** (cross-session contamination), hit further nit-wrapper opacity (below), and **correctly STOPPED before committing/pushing** — recognizing it would otherwise push the sensitive `demeter_excavation` drive-wipe doc bundled into a "skills sweep" push. **The differential matters:** Session 2 paused at the anomaly; Session 1 pushed through the uninformative gate. The lesson is behavioral *and* structural.

**Net consequence:** `3cf94eb8` is a **local, NOT-pushed, reversible** commit bundling two unrelated workstreams + an unintended live `~/.zshenv` deploy. No data lost, nothing pushed, Shannon unaffected. User chose to leave the commit as-is (the bundling per se is a non-issue to the user). **The real problems to fix are the deploy side-effect + the concurrency race — not commit-scope tidiness.**

### Observed nit behaviors / sharp edges (for the implementer)

1. **`nit commit` is not staged-set-scoped.** No template drift anywhere → thin "plain files only" commit (≈ honors intended scope; `b643880c` proves it). ANY tracked template drifted anywhere in the work-tree → **template-mode**: commits the FULL modified set (`-a`-like, ignoring explicit `nit add`) AND renders+deploys all templates.
2. **`nit commit` template-mode couples persist (git) with deploy (render templates to the live machine).** A commit can therefore deploy *another session's half-finished template edit* prematurely. This coupling is the single most dangerous property.
3. **The session-anchor ack-gate blocks-with-instruction but does not inform** (no "this will commit N files / deploy M templates to live targets" preview). Per the user's own *Auto-resolving-gates-over-blocking-gates* directive, an uninformative blocking gate gets reflexively re-run — which is exactly what happened.
4. **The shared bare repo + `$HOME` work-tree + single git index is global mutable state with no concurrency control.** N CC sessions are mutually unaware. Interleaved `nit add` (session A) / `nit commit` (session B) → contamination. Symptom both sessions independently observed: `nit status` showing `0 staged` where a prior `nit add` in the same session reported many staged — i.e. **the index appears to mutate/reset between separate nit invocations** under concurrent use. Implication: a non-atomic add-then-commit is unsafe under concurrency.
5. **Wrapper opacity defeats scope-verification (secondary but critical).** `nit commit` clap parser supports `-m` only (no `-F`/stdin body). `nit diff`/`nit log -- <pathspec>` (and likely others) eat the `--` separator (clap), returning **false-empty** pathspec-scoped output. Raw-git against the bare repo reads a different/again-inconsistent index. **Result: neither session could reliably answer "what exactly will this commit contain / deploy?"** — the core safety-verification primitive is missing.

### Archaeology reframe (founding-spec — read before designing the keystone)

A founding-spec read (`~/dotfiles/.claude/specs/nit/requirements.md` US-5 + EARS, `design.md`, this `MIGRATION_CHECKLIST.md`) materially reframes the fix:

1. **Concurrency was nit's *founding motive*** (requirements.md:5/15 — "16 concurrent Claude Code sessions"). This contamination is the exact class nit was built to kill; it solved chezmoi's source/target contamination but a different concurrent-session contamination slipped in via the commit path.
2. **`commit → render → deploy` is an EXPLICIT founding requirement** (requirements.md:43 EARS). So "decouple commit from deploy" is a **design REVERSAL of a founding requirement**, not a bug-fix — do not frame it as cleanup.
3. **The crux — the spec already says what we want; the implementation diverged.** AC-5.2: the ack-gate must key off **staged** template sources. AC-5.7: **plain-file-only commits skip all ack checks (zero friction)**. The observed bug: when a template is *modified-but-not-staged* (the canonical concurrent-session case), the impl abandons staged-scope and goes whole-tree `-a`-like + deploys. Commits `b643880c`/`99b6503c` ("plain files only") prove the spec-intended path works when no template is modified anywhere. **Most of the danger is implementation-vs-spec divergence, not a missing design.** ⚠ A future agent reading `requirements.md` US-5 should know the impl currently diverges from AC-5.2/AC-5.7 (recommended cross-ref note for the dotfiles pass).
4. **"No lock file" (AC-5.9) was *ack-scoped*, not index-scoped** — it never reasoned about git-index contention between sessions. An index lock would not violate founding intent, but is likely unnecessary once the keystone lands.
5. **The footgun was never anticipated anywhere** — concurrency was treated only as a *migration-time* hazard (close all sessions), never a steady-state commit hazard. Genuinely new evidence.

### Behavioral / CLAUDE.md directives
*(Edits land in `~/.claude/CLAUDE.md`, not nit code. Forging requires loading the `prompt-engineering` skill first, per the MANDATORY forge rule.)*

- ✅ **Commit-posture recalibration** — LANDED (`~/.claude/CLAUDE.md` § Git Workflow Protocol "Commit caution scales with reversibility"; dotfiles `99b6503c`). Local-unpushed-commit = low ceremony; rigor concentrates at push / clobber / deploy.
- [ ] **Unexpected-gate-is-information** — an ack-block / drift-abort / surprising status during commit signals a concurrent session or pending deploy. Investigate before re-running; never reflex-clear. (Session 1's specific behavioral failure.) Not yet forged; candidate to fold as a clause of the recalibration.
- [ ] **Atomic add+commit discipline** — always `nit add … && nit commit …` in ONE shell chain (the inter-invocation window is the concurrency race, observed-behavior #4). Partly embodied in practice; not yet a forged directive.

### nit code/design changes

- [ ] **Honor-staged-scope keystone (spec-CONFORMANCE to AC-5.2/AC-5.7 — NOT a redesign).** *The primary fix.* Make `nit commit` commit exactly the explicitly-staged paths; key the ack-gate off **staged** template sources; plain-only-staged → the zero-friction path even when an unrelated non-staged template is modified. This alone neutralizes the contamination. RED-GREEN against AC-5.2/AC-5.7. **Safety-critical `cmd_commit` surgery — fresh focused session, not marathon-tail.**
- [ ] **Re-scope deploy to staged (NOT "decouple" — that reverses founding EARS:43).** A commit deploys ONLY the templates it actually commits (staged), never a concurrent session's non-staged in-flight template. Honors EARS:43 *and* AC-5.2; kills the deploy-side-effect footgun without reversing the founding requirement. Pairs with the keystone.
- [ ] **nit-transparency fix** (additive — contradicts no spec; companion to the keystone). Support `-F`/stdin commit body; stop the clap wrapper eating `-- <pathspec>` so `nit diff/log/status -- <path>` are reliable; make `nit status` / `nit diff --cached` an authoritative pre-commit preview (scope-verification depends on it, observed-behavior #5).
- [ ] **Index advisory lock — DEMOTED.** Once commits are staged-scoped, interleaving degrades to a benign partial, not a sweep+deploy. Do only if the keystone proves insufficient.
- [ ] **Session-scoped changeset (optional).** Extend the session-anchor concept to commit scoping so two sessions each told "commit my work" can't bundle each other's.
- ✅ **Forward-only-sync** — IMPLEMENTED (`3d6bfec`, local, not pushed). See Status block. (This is the resolution of the runtime-noise files referenced in the Context above.)
- ✅ **Clear the pre-existing clippy toolchain-drift — DONE** (this commit, separate scoped, local). See Status block; was `--all-targets` test-only, never the CI/push blocker it had been framed as (`cargo clippy -- -D warnings` was already green).

**Recommended order (archaeology-reframed):** the **honor-staged-scope keystone** + **re-scope-deploy-to-staged** (the spec-conformance pair — *this* is the structural keystone, NOT the old "decouple" framing) → **nit-transparency fix** → clear the clippy blocker (independently, anytime, before push) → index lock only if needed → session-scoped-changeset optional. The keystone pair = fresh focused session with full RED-GREEN.

### Cross-references
- **Correct-behavior reference:** `b643880c` + `99b6503c` (4-/2-file "plain files only" path — what `nit commit` *should* do; proves the spec-intended path works absent template drift).
- **Failure exemplar:** `3cf94eb8` (template-mode, 21-file + 17-template live-deploy). All local, unpushed.
- **Forward-only-sync implementation:** nit `3d6bfec` (local, unpushed).
- Originating Shannon context: `~/dotfiles/TODO.md` § Shannon (RESOLVED) + `~/dotfiles/docs/demeter_excavation_2026_05_14.md` § Status.
- Concurrent workstream: `~/.claude/skills/*` pushy-description sweep + `~/dotfiles/templates/.zshenv.tmpl` + dotfiles TODO skill-loading-enforcement section.
- Harmonize with dotfiles directives: *Sacred drift-safety*, *Auto-resolving gates over blocking gates*, *Co-triage bulk deletions*, *Aesthetic-as-decision*, the canonical-wrapper-map, *Personal/secret-containing data → NEVER push*, and the new *Commit caution scales with reversibility*.
