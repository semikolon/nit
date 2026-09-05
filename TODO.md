# nit — TODO

> nit's forward-work has historically been tracked in `docs/MIGRATION_CHECKLIST.md`
> (v2 drift-promotion to source, fleet rollout mechanism, etc.). This file is the
> project TODO. The entry below was logged here per explicit user request
> (2026-05-17) as a **self-contained, incident-driven task cluster** for a
> dedicated future CC session — it deliberately carries full incident context
> because the redesign tasks cannot be designed correctly without it.

---

## 🆕 forward_only redesign + Drift Steward — design doc (2026-07-11)

Exhaustive design + prior-reasoning capture at **`docs/forward_only_drift_steward_design_2026-07-11.md`**.
Core: `forward_only` currently freezes the git copy + snapshots locally (no audit history, latest
never reaches the repo, still trips TCC/raw-`add -A`). Fredrik wants config drift **in** git/nit
history but with zero cognitive load — delegated to Claude in-session with full context (a "Drift
Steward"), never a blind nightly job (see the 2026-05-17 concurrent-session incident below for
why). Includes the 5-file audit (superwhisper was mis-filed → plain-track), the 5-bucket taxonomy,
the two real constraints (per-machine namespacing / never-push-secrets), the `skip-worktree`
finding, and open decisions. Immediate low-risk action: revert superwhisper out of `forward_only`.

**Founding-intent finding (2026-07-12, doc §5.5):** forward_only was MEANT to reach git (flush =
commit, originally pushable); frozen-baseline is a double-regression from a false "target always
ahead" premise that breaks only for per-machine-divergent files. The Steward *restores* founding
intent via per-machine namespacing, not new scope.

### ✅ Partly BUILT 2026-09-05 — what exists vs what is still design

Shipped in `src/drift_triage.rs` (in master + installed on macmini; **fleet rollout
deliberately HELD** at Fredrik's request while another session debugs the Mac Mini —
no `.nit-version` bump, so every other machine still runs the previous nit):

- **The staleness gate** — *do these exact bytes already exist in this path's git
  history?* Stale / Unique / Deleted / Unknown, with the matching commit named.
  This is the FIRST piece of the classifier and currently the only piece.
- **The deadlock detector** — drift plus no successful sync for ≥3 days is a
  distinct condition with the OPPOSITE remedy (discard and pull, never commit).
- **The ntfy escalation** — once on entry, then at most weekly, to `fleet-sync`.

**Still design, and this IS the Steward work:** the rest of the classifier (for a
file that is NOT stale — tuned setting, machine-written junk, per-machine-divergent,
or secret-bearing → commit / commit-locally / skip / ask), the trigger and timing,
and per-machine routing.

### 🆕 Two findings from the MERIAN recovery that change the design (doc §11, §12)

- [ ] **Add a SILENCE condition, not just a refusal condition.** The deadlock
  detector fires on an abort. A machine whose nightly simply stops running never
  aborts and so is invisible: **Shannon's last successful nit sync was 2026-06-27**,
  with zero drift and status `ok`. Condition wanted: *last success older than N days
  regardless of outcome*.
- [ ] **hemma overlay drift is the LOWER LAYER of the same pipeline, not a second
  surface** (doc §12 — corrects an earlier same-day claim). 692 files under
  `dotfiles/system/` are nit-tracked, so the Steward already owns the overlay
  SOURCE layer for free; only the deployed-copy layer is hemma's. Judgment
  generalises, plumbing does not → one classifier, two adapters, and
  `shannon-drift-watch` shrinks to a thin feed. **The prize is one escalation path**:
  on 2026-09-05 its hand-rolled alert was found to have been undeliverable for
  months across five independent breaks, while a second alert was being built for
  the nit side.

### Decisions to pin first (doc §11 — rationale there)
- [ ] Nudge fires in **any** live session vs **designated** session only.
- [ ] Per-machine-history mechanism: namespaced local ref vs isolated local repo vs `skip-worktree`.
- [ ] Pure-runtime caches: local-only git history vs gitignore.
- [ ] Adopt `skip-worktree`? (kills raw-`add -A` loophole + TCC stat; reconcile vs the `3d6bfec`
  "detect all drift" invariant first).
- [ ] codex `config.toml` templatization: now vs deferred (8 secret-ish MCP tokens → careful).

### Build (nit source — RED-GREEN + fleet-roll via `.nit-version`/`rebuild-nit`)
- [ ] `nit-drift-steward-nudge` sibling hook: UserPromptSubmit, conditional-silent, reads durable
  drift (`nit status` scoped to declared set / `last-sync.json` `drift_files`), surfaces at next turn.
  (~90% shape-clone of `fleet-drift-nudge`; different body — see doc §9b correction.)
- [ ] Per-machine namespacing mechanism (whichever the decision picks) so divergent caches reach git
  without cross-machine conflict.
- [ ] Steward classify+commit: meaningful-vs-cosmetic gate + machine-marker grep + no-op hash skip;
  commits via session-intent-scoped `nit commit` (explicit paths, **never** `add -A`).
- [ ] Nightly `nit update` = **detect + notify only** (ntfy from `last-sync.json` `drift_files`;
  never auto-commit context-blind — doc §2).

### Test targets (doc §9b — pin these behaviors)
- [ ] Meaningful edit to a runtime file gets committed (not skipped).
- [ ] Cosmetic-only rewrite (reordered JSON, bumped timestamp) is skipped/squashed.
- [ ] Secret-bearing runtime file committed **locally**, **never pushed** (pre-commit hook = gate).
- [ ] Per-machine-divergent file commits to its namespaced ref with **no cross-machine conflict**
  on another machine's pull (headline regression, mirrors `02c30db`).
- [ ] Steward **never** runs whole-index `add -A` (assert scoped commit).

### Independent low-risk action (no build needed)
- [ ] Revert superwhisper out of `fleet.toml` `forward_only` → plain-track + push (backs out the
  mitigation-as-deferral in nit-bare-repo commit `16104d82`).

### Docs housekeeping
- [ ] Commit the untracked design doc `docs/forward_only_drift_steward_design_2026-07-11.md`.

---

## Concurrent-session commit safety + commit/deploy decoupling — incident-driven (2026-05-17)

**Status (2026-05-17, partially actioned):** committed `9b49e37`. Since then, two of these landed and the rest were *reframed by founding-spec archaeology* (see "Archaeology reframe" below — it materially changes the approach):
- ✅ **Commit-posture recalibration** — LANDED as the "Commit caution scales with reversibility" subsection in `~/.claude/CLAUDE.md` § Git Workflow Protocol (dotfiles commit `99b6503c`).
- ✅ **Forward-only-sync feature** — IMPLEMENTED + verified (nit commit `3d6bfec`, LOCAL, not pushed; 189 tests incl. 7 RED-GREEN + 2 SAFETY assertions; new code clippy-clean). Resolves the perpetual runtime-noise + nightly-sync-ABORT for the decisions-state/cache + spela-config files. Activation gates (USER): declare the 3 paths in `fleet.toml [sync] forward_only`, then `.nit-version` bump. (Was a deferred MIGRATION_CHECKLIST v2 item — now done; that entry trued.)
- ✅ **clippy `--all-targets` test-target hygiene — CLEARED** (this commit; separate scoped commit, local, not pushed; 189 tests green before+after). **Reconciliation correction to the prior framing:** these 11 were NOT a CI/push blocker. CI runs `cargo clippy -- -D warnings` (lib/bin targets only, no `--tests`) and was already GREEN — the lib/bin clippy errors had been cleared earlier in `f2c92ee`. The 11 remaining were `#[cfg(test)]`-only lints surfaced solely by `cargo clippy --all-targets` under Rust 1.93's stricter test-code linting. Cleared deliberately as a SEPARATE scoped commit (NOT bundled into the keystone — Co-triage / commit-caution). Sites: config.rs ×1 (needless borrow), encrypt.rs ×4 (`&[x.clone()]`→`std::slice::from_ref`), syncbase.rs ×4 (nested-if→let-chains), trigger.rs ×2 (useless `vec!`, field-reassign-with-default).
- ✅ **Honor-staged-scope keystone + re-scope-deploy-to-staged — IMPLEMENTED + verified** (this commit; LOCAL, not pushed; full RED-GREEN, 198 tests incl. 7 keystone + 2 store; end-to-end git `:/`-pathspec partial-commit proven against a temp bare repo simulating the `3cf94eb8` race → the 21-file bundle becomes a clean 1-file session-scoped commit, the other session's work preserved in-index, zero cross-session deploy). **Design decision (user-pinned via consult, per *Consult before shared-infrastructure design changes*):** *Session-intent scoping* — `nit add` records this session-anchor's staged paths (mirrors the ack store); `nit commit` scopes commit + template-deploy to ITS OWN session's set ∩ live index. Detail in "nit code/design changes" + the archaeology-reframe correction below.
- ✅ **nit-transparency fix — IMPLEMENTED + verified** (separate scoped commit; LOCAL, not pushed; full RED-GREEN, 208 tests incl. 10 new + end-to-end smoke through the real binary). Three parts: (1) `-F <file>` / `-F -` (stdin) commit body, `-m`/`-F` mutually exclusive, empty-message rejected — pure `resolve_commit_message` (6 RED-GREEN tests); (2) **the `--`-eating fix** — pure-passthrough subcommands (log/diff/show/push/mv/rm/reset/reflog/branch) are routed to git in `main()` BEFORE `Cli::parse()`, so `nit log -- <path>` etc. preserve the `--` pathspec separator verbatim (clap's `trailing_var_arg` was stripping it → false-empty pathspec scope = the exact scope-verification blindness of observed-behavior #5) — pure `passthrough_subcommand` (4 RED-GREEN tests) + smoke proved `nit log -- a.txt`/`-- b.txt` pathspec-filter correctly; (3) **`nit status -v` session-intent commit-scope preview** — reuses the SAME `plan_commit_scope` + SAME `git diff --cached --name-only` cmd_commit uses, so it answers "what exactly will `nit commit` include/deploy?" and "what's excluded as another session's" without lying. Separate scoped commit (NOT bundled with keystone `02c30db` or clippy `51e002a` — Co-triage / commit-caution).

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
- ✅ **Unexpected-gate-is-information — FORGED** (2026-05-17). Now a MANDATORY behavioral directive in `~/.claude/CLAUDE.md` § Development Philosophy gate-cluster (between *Try the normal path first* and *Auto-resolving gates over blocking gates*): an unpredicted ack-block / drift-abort / hook refusal / disagreeing status at a gated op = information about an unmodeled state (concurrent session, pending side-effect, stale tooling) → investigate before re-running, never reflex-clear. `prompt-engineering`-skill audited (all 9 techniques + a step-4 over-fire caveat so it does NOT freeze on nit's *legitimate* same-session two-call ack). LIVE for all future sessions (plain-file CLAUDE.md = no apply step). It is the *behavioral* defense-in-depth complementing the *structural* keystone (`02c30db`). **Tracking-commit of the dotfiles `~/.claude/CLAUDE.md` change rides the user's DEFERRED dotfiles pass** — deliberately NOT ad-hoc `nit commit`'d now: the installed `nit` is still the pre-keystone binary (session-intent scoping is in `~/Projects/nit` + pushed to `semikolon/nit`, but NOT yet fleet-deployed via `.nit-version`/`rebuild-nit`), so committing the shared bare repo mid-session with un-hardened tooling is exactly the risk this directive + the incident warn against — dogfooding the lesson. **The incident-driven cluster is now fully closed** (only the DEMOTED index-lock rejected-alternative remains, contingent on a real keystone-insufficiency failure).
- ✅ **Atomic add+commit discipline — OBSOLETE; deliberately NOT forged** (keystone-resolved 2026-05-17). The add→commit inter-invocation window (observed-behavior #4) is the race this discipline guarded; **session-intent scoping structurally removed the need** — `nit commit` scopes to what THIS session recorded regardless of timing, so a concurrent `nit add` in the window can no longer contaminate. Forging a *mandatory discipline-rule* here would directly contradict the user's own *Auto-resolving gates over blocking gates* directive ("a gate that requires discipline is a gate that will fail; redesign it to resolve transparently" — the keystone IS that redesign). Recorded so a future session doesn't "helpfully" re-add a now-counterproductive directive. (Chained `nit add && nit commit` stays mildly tidy practice — no longer load-bearing safety.)

### nit code/design changes

- ✅ **Honor-staged-scope keystone — DONE via Session-intent scoping** (this commit). `nit add` records the work-tree-relative paths THIS session-anchor staged (delta vs pre-add index) into `~/.local/share/nit/staged/<anchor>.json` (mirrors the ack store: `record_session_staged`/`read_session_staged`/`clear_session_staged`/`prune_dead_staged` + pure `merge_staged`). `cmd_commit` calls the pure `plan_commit_scope(session_staged, index_staged, template_source_rels)` → commits exactly `session ∩ index` via `git commit -- :/<path>` (the `:/` work-tree-top magic = CWD-independent; closes sharp-edge #5), ack-gates only in-scope template sources (AC-5.2), takes the zero-friction path when only plain files are in scope even if another session staged/modified a template (AC-5.7), clears the session record on success. 7 RED-GREEN tests (headline `scopes_out_concurrent_session_index_entries` = the `3cf94eb8` regression).
- ✅ **Re-scope deploy to staged — DONE** (same commit; substrate of the above). Deploy loop iterates ONLY `plan.deploy_mapping_idx` (templates whose source is in THIS session's scope) — never `&mappings` (all), never a concurrent session's in-flight template. EARS:43 honored (a commit still renders+deploys its OWN templates); the deploy-side-effect footgun is closed. Empty-scope → clean error (never a bare `git commit`). No session record (raw `git add` / fresh session) → legacy whole-index + visible bypass warning (backward-compat).

> **⚠ Archaeology-reframe correction (recorded during implementation, 2026-05-17 — docs mustn't lie).** The reframe asserted the literal "spec-conformance pair" (staged-index pathspec + deploy-rescope) *"alone neutralizes the contamination"* and treated session-scoped-changeset as a separable *"(optional)"* item. A code-level trace against `3cf94eb8` (where Session 2's template *was* in the SHARED index) showed the literal pair does **not** neutralize the actual incident — it only shrinks blast radius while still bundling + live-deploying the other session's staged template. The faithful reading of requirements.md US-5 ("commit exactly the explicitly-staged paths" = staged *by this session*) **is** session-intent scoping; "(optional) session-scoped changeset" was the same mechanism described twice, not a deferral. User consulted + pinned Session-intent scoping. **Index advisory lock disposition (2026-05-17):** stays DEMOTED and is NOT a TODO item — session-intent scoping fully neutralizes the race (verified end-to-end against the `3cf94eb8` simulation, where interleaving would otherwise degrade to a sweep+deploy). It is a *rejected alternative*, reconsidered ONLY if a real-world failure proves the keystone insufficient (folded here from a former `[ ]` bullet — it was a contingency, not a plan; TODO.md tracks plans).
- ✅ **nit-transparency fix — DONE** (separate scoped commit; see Status block). `-F`/stdin commit body (pure `resolve_commit_message`); pre-clap passthrough interception preserves `-- <pathspec>` verbatim (pure `passthrough_subcommand`); `nit status -v` shows the session-intent commit-scope preview via the same `plan_commit_scope`. 10 RED-GREEN tests + end-to-end binary smoke. Note: `nit status -v` is the authoritative preview surface; `nit diff --cached` now also passes `--`/pathspec through correctly via the interception.- ✅ **Session-scoped changeset — DONE (this WAS the faithful keystone, not "optional").** Reclassified during implementation: extending the session-anchor concept to commit scoping IS the honor-staged-scope keystone (see correction note above), implemented this commit. The "(optional)" framing was an archaeology-reframe miss.
- ✅ **Forward-only-sync** — IMPLEMENTED (`3d6bfec`, local, not pushed). See Status block. (This is the resolution of the runtime-noise files referenced in the Context above.)
- ✅ **Clear the pre-existing clippy toolchain-drift — DONE** (this commit, separate scoped, local). See Status block; was `--all-targets` test-only, never the CI/push blocker it had been framed as (`cargo clippy -- -D warnings` was already green).

**Recommended order (updated 2026-05-17):** ✅ clippy hygiene (`51e002a`) → ✅ **honor-staged-scope keystone + re-scope-deploy-to-staged via Session-intent scoping** (`02c30db`; RED-GREEN + end-to-end verified) → ✅ **nit-transparency fix** (this commit; RED-GREEN + end-to-end smoke) → ✅ obsolescence pass (index-lock + atomic-add-commit retired as keystone-resolved; *unexpected-gate-is-information* kept) → ✅ **PUSHED to `semikolon/nit` master, 2026-05-17** (PUBLIC OSS repo; trufflehog-clean — 0 verified / 0 findings of any kind; clean fast-forward, no force; 208 tests + `cargo clippy -- -D warnings` + `--all-targets` + `cargo fmt --check` all green). Index lock = rejected alternative; revisit only on a real keystone-insufficiency failure (see correction note above).

### Post-keystone fleet-rollout hardening (2026-05-18)

Surfaced + fixed while deploying the keystone fleet-wide (all pushed; Mac Mini runs `cd1d18a`):
- ✅ **`nit sync` no longer git-commits** (`cd1d18a`) — snapshots forward-only paths to `~/.local/share/nit/forward-only/` (restic-covered, local-only). Kills the push-lineage → fleet-pull-conflict landmine. MIGRATION_CHECKLIST forward-only § trued + residual caveat recorded there.
- ✅ **`nit commit -m` repeatable like git** (`ee041f9`) — `Vec<String>`, `\n\n`-joined paragraphs; was a single-value clap footgun.
- ✅ **`rebuild-nit` no_local_build gate** (dotfiles `bfa4362f`) — shannon+turing flagged `no_local_build = true` in fleet.toml; rebuild-nit is **correct-by-construction** structurally incapable of `cargo install` on them (fetches Darwin-cross-staged aarch64, else skip-loud; stale-but-running is safe, frozen is not). Strong machines unchanged (verified per-machine vs real fleet.toml; `bash -n` clean).
- ✅ **Push-gate directive relaxed** (`~/.claude/CLAUDE.md` § Git Workflow) — private-repo non-secret push = low-ceremony just-push; full rigor only for public / secret-bearing / sensitive-aggregate; plaintext-credential invariant always.

**Durable follow-up (the rebuild-nit fix is an INTERIM — *Fix the system, never work around it*):** Darwin-internal cross-stage is fleet-build infra. End-state = the **GitHub-Actions release-artifact path design.md already intends** (`release.yml` cross-builds aarch64/x86_64/arm64-darwin; make `.nit-version` bumps produce/point to a downloadable artifact so NO fleet box compiles, Darwin included). Not built; tracked here.

**Per-host fleet propagation (2026-05-18 — largely COMPLETE):** Mac Mini / MERIAN / Darwin all on `cd1d18a`. Darwin (prod-router) upgraded after a live resource-preflight + lossless drift-resolve (2 tracked files `git checkout`-discarded toward origin after verifying the secret `.age` was plaintext-identical to the SSoT — the apparent "fork" was a stale-local-HEAD diff artifact, not a real divergence); its `rebuild-nit` then cross-staged `nit-<cd1d18a-fullsha>-aarch64-unknown-linux-gnu` (7.8 MB, exec) to `darwin:~/.local/share/nit/prebuilt/`, triggers 2/2, sync `ok`. **Shannon: unblocked, pending its own session** — run the one-time forward-only drift-resolve + `nit update`; its `no_local_build` branch scp-fetches the now-staged aarch64 prebuilt (zero compile). **Turing: deferred** (unplugged; identical fetch path on reactivation — prebuilt already waiting). The pre-fix `nit sync` snapshot-commits-on-origin landmine still gates each machine's FIRST post-upgrade pull (drift-resolve forward-only paths first; one-time) — MERIAN + Darwin cleared, Shannon + Turing remain. Procedure: `docs/MIGRATION_CHECKLIST.md` forward-only "Shipped reality (2026-05-18)" note.

**✅ nit secret-drift heuristic — direction-aware (shipped 2026-05-18):** `9b29f87`'s "unflushed manual edit detected" abort fired twice during the cd1d18a rollout when the true cause was "source advanced upstream, deployed target is stale" — NOT a human edit. Fixed: `DriftClass` {`StaleTarget` | `LikelyUnflushedEdit` | `Ambiguous`} + pure `classify_env_drift` + shared `drift_guidance`, wired through `cmd_apply`/`cmd_update`/`bootstrap`. The abort now names the direction (target-missing-only ⇒ stale ⇒ `--force-drift` SAFE; target-extra/changed ⇒ genuine edit ⇒ `nit encrypt`). 8 RED-GREEN tests (incl. the exact MERIAN stale-target case); 219 pass, clippy/fmt clean.

### Cross-references
- **Correct-behavior reference:** `b643880c` + `99b6503c` (4-/2-file "plain files only" path — what `nit commit` *should* do; proves the spec-intended path works absent template drift).
- **Failure exemplar:** `3cf94eb8` (template-mode, 21-file + 17-template live-deploy). All local, unpushed.
- **Forward-only-sync implementation:** nit `3d6bfec` (local, unpushed).
- Originating Shannon context: `~/dotfiles/TODO.md` § Shannon (RESOLVED) + `~/dotfiles/docs/demeter_excavation_2026_05_14.md` § Status.
- Concurrent workstream: `~/.claude/skills/*` pushy-description sweep + `~/dotfiles/templates/.zshenv.tmpl` + dotfiles TODO skill-loading-enforcement section.
- Harmonize with dotfiles directives: *Sacred drift-safety*, *Auto-resolving gates over blocking gates*, *Co-triage bulk deletions*, *Aesthetic-as-decision*, the canonical-wrapper-map, *Personal/secret-containing data → NEVER push*, and the new *Commit caution scales with reversibility*.
