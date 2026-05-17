# nit — TODO

> nit's forward-work has historically been tracked in `docs/MIGRATION_CHECKLIST.md`
> (v2 drift-promotion to source, fleet rollout mechanism, etc.). This file is the
> project TODO. The entry below was logged here per explicit user request
> (2026-05-17) as a **self-contained, incident-driven task cluster** for a
> dedicated future CC session — it deliberately carries full incident context
> because the redesign tasks cannot be designed correctly without it.

---

## Concurrent-session commit safety + commit/deploy decoupling — incident-driven (2026-05-17)

**Status:** logged for a dedicated future CC session; user will triage. NOT started. This note is uncommitted at time of writing (the originating session was Shannon-focused and was asked only to record, not action, this).

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

### Tier 1 — Claude-behavioral / CLAUDE.md directives
*(The actual edits land in dotfiles/global `~/.claude/CLAUDE.md`, not nit code; tracked here for unified triage. Forging any of these requires loading the `prompt-engineering` skill first, per the MANDATORY forge rule.)*

- [ ] **T1.1 — Recalibrate commit posture.** Local, unpushed, all-legitimate-work commit = low ceremony (reversible, nothing left the machine). Concentrate rigor at the boundaries that actually bite: (1) `nit push` — verify no sensitive/unintended content leaves the machine (the `demeter_excavation`-in-a-skills-push near-miss is the *valid* fear); (2) clobber/loss (Sacred drift-safety already covers); (3) deploy/trigger side-effects of `nit commit`/`nit apply`. The base "stage specific files" rule's spirit is sensitive/unintended-content-reaching-a-remote, not local-scope perfectionism.
- [ ] **T1.2 — An *unexpected* nit gate is information, not ceremony.** An ack-block, drift abort, or surprising status during commit is a likely signal of a *concurrent session* or *pending template deploy*. Investigate before re-running; never reflex-clear it. (This is the specific behavioral failure of Session 1.)
- [ ] **T1.3 — Atomic `nit add … && nit commit …` in ONE shell chain, always.** Never split add and commit across separate tool calls — the inter-invocation window is the concurrency race (finding #4).

### Tier 2 — nit code/design changes

- [ ] **T2.1 — (highest leverage) Decouple commit from deploy.** `nit commit` persists to git only; template render/deploy stays exclusively `nit apply`. Removes the dangerous "a commit also deploys another session's in-flight template" coupling (finding #2). This is the recommended primary fix.
- [ ] **T2.2 — Honor the explicit staged set OR informative preview+confirm.** `nit commit` should commit exactly the explicitly-staged paths. If it will commit-all and/or deploy, print `WILL commit N files: […]` + `WILL deploy M templates to live targets: […]` and proceed only on confirm. Redesign the uninformative ack-gate into this informative gate (per *Auto-resolving-gates-over-blocking-gates*).
- [ ] **T2.3 — Work-tree/index advisory lock.** `nit add/commit/apply` acquire an advisory lock; a concurrent nit op waits or refuses with a clear message naming the holding session/PID. Direct root-cause fix for the concurrency race (finding #4); lighter-weight than standing up `mcp-agent-mail`.
- [ ] **T2.4 — Wrapper transparency.** Support `-F`/stdin commit body; stop the clap wrapper eating `-- <pathspec>` so `nit diff/log/status -- <path>` are reliable; make `nit status` / `nit diff --cached` an authoritative pre-commit preview. Scope-verification depends on this (finding #5).
- [ ] **T2.5 — (consider) Session-scoped changeset.** Extend nit's existing session-anchor concept (used for acks) to commit scoping: a mode that commits only files touched by the calling session/agent identity, so two sessions each told "commit my work" cannot bundle each other's changes.

**Recommended order:** Tier 1 (T1.1–T1.3, immediate, free) → T2.1 (decouple commit/deploy, the structural keystone) → T2.2/T2.3 → T2.4 → T2.5.

### Cross-references
- **Correct-behavior reference:** commit `b643880c` (4-file, "plain files only" path — what `nit commit` *should* do).
- **Failure exemplar:** commit `3cf94eb8` (template-mode, 21-file + 17-template live-deploy). Both **local, unpushed**.
- Originating Shannon context: `~/dotfiles/TODO.md` § Shannon (RESOLVED) + `~/dotfiles/docs/demeter_excavation_2026_05_14.md` § Status.
- Concurrent workstream: `~/.claude/skills/*` pushy-description sweep + `~/dotfiles/templates/.zshenv.tmpl` + dotfiles TODO skill-loading-enforcement section.
- Adjacent dotfiles directives this should harmonize with: *Sacred drift-safety*, *Auto-resolving gates over blocking gates*, *Co-triage bulk deletions*, *Aesthetic-as-decision*, the canonical-wrapper-map, and the "Personal/secret-containing data → NEVER push" directive (the push-boundary rigor in T1.1).
