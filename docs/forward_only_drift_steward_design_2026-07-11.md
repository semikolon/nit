# forward_only redesign + the Drift Steward — design doc

**Date:** 2026-07-11
**Status:** design / handoff. No code changed. Feeds a nit-side decision + build.
**Author:** Claude (Opus 4.8) with Fredrik, in a sarpetorp session.
**Why it lives here:** the reasoning was excavated from nit git history + the personal
corpus (`mannaminne`) during a long session; capturing it verbatim so a fresh nit-repo
session can act without re-excavating. Handoff target, not a chronicle.

---

## 0. TL;DR

`forward_only` today means **"freeze the git copy at a baseline; snapshot runtime changes to a
local restic-covered dir; never re-commit, never push."** That was a sound fix for genuine
per-machine *runtime state* (caches), but it has three consequences Fredrik dislikes:

1. A config that merely *drifts* (an app rewrites it) gets mis-filed here and its **latest
   contents never reach the repo** — a rebuilt machine provisions the stale baseline.
2. There is **no git audit history** of the runtime changes (only a restic snapshot).
3. The residual drift still trips `git status` (→ TCC/Documents prompt on Mac) and can still
   be swept into git by a **raw `git add -A`** from any session.

Fredrik's actual want, stated verbatim in-session: *"All I've personally wanted to avoid was
having to manually allocate cognitive cycles toward this type of drift. I've wanted to delegate
it to AI (with full context… that can surface questions to me; Claude is optimal) but never
minded it being a part of my git/nit history."*

**The design that delivers that:** a **Drift Steward** — Claude, in-session, right after drift
occurs, with full context — commits config drift into **git/nit history** (per-machine-scoped
where the file diverges; local-only where it carries secrets), writes honest messages, and
surfaces only genuine ambiguity to Fredrik. This is ~80% already the *Autonomous-commit
discipline* directive; the net-new is timing/scoping mechanics + a cosmetic-vs-meaningful gate
+ per-machine routing. It is explicitly **NOT** an unattended nightly job (see §3 for why).

---

## 1. What `forward_only` actually does today (verified from source + git)

Two commits define it:

**`3d6bfec` — feat(sync): forward-only sync (original spec).**
> "Runtime-mutated tracked files (decisions state/cache, spela config) are dirty BY DESIGN;
> nit must not let them trip the safety-critical pre-pull ABORT or pollute the 'modified'
> status line, nor deploy source->target over them."
- `config.rs`: `[sync] forward_only` path list + `porcelain_path` / `is_forward_only` /
  `filter_forward_only_drift`.
- `cmd_update`: filters forward-only **out of drift at the call site**; `detect_pre_pull_drift`
  **left UNTOUCHED** — "it must keep detecting all drift — it prevented the 2026-05-04 clobber."
- `cmd_status`: forward-only excluded from the scary "modified" count, surfaced calmly.
- `nit sync`: originally a pathspec-scoped flush **commit** (never `-A`, not pushed).

**`cd1d18a` — fix(sync): nit sync snapshots locally instead of git-commit.**
> "A nit sync commit on master rode the push lineage to origin and would merge-conflict every
> fleet machine's pull of its own runtime-drifted forward-only files… nit sync now atomically
> snapshots present forward-only paths to `~/.local/share/nit/forward-only/` (restic-covered) —
> local-only, never a commit/push; origin's forward-only files stay static (bootstrap still
> seeds from baseline)."

**Net current behavior:**
- The git copy of a `forward_only` file is a **frozen baseline** (whatever was last committed,
  e.g. at bootstrap). nit **never re-commits it.**
- `nit sync` writes the live version to `~/.local/share/nit/forward-only/<path>` (local,
  restic-covered, never pushed). Source: `syncbase.rs:63` `write_forward_only_snapshot`.
- `git status` **still stats** the file (detection is deliberately unfiltered) → so the file is
  still *read* on every `nit update` (relevant to the TCC angle, §9).
- **No `skip-worktree`** is used anywhere in nit (verified: `grep skip.worktree` → none).
- Consequence: a fresh `nit apply` on a rebuilt machine provisions the **stale baseline**, and
  a raw `git add -A` (bypassing nit's own scoping) can still stage the live version.

---

## 2. Why unattended / context-blind drift-commit is risky (the prior reasoning)

Excavated from git + `mannaminne`. This is the load-bearing constraint on the design.

**`3cf94eb8` — the 2026-05-17 concurrent-session incident.**
> "Two CC sessions sharing one bare repo + index: Session 1's `nit commit` bundled Session 2's
> 17 staged skills + `.zshenv.tmpl` into `3cf94eb8` AND live-deployed Session 2's in-flight
> `~/.zshenv` (deploy loop iterated ALL templates; bare `git commit` committed the whole shared
> index)."

**`02c30db` — the fix: session-intent scoping.**
- `nit add` records this session-anchor's staged paths to
  `~/.local/share/nit/staged/<anchor>.json`.
- `plan_commit_scope`: commit scope = session-recorded ∩ live index; deploy scope = only
  templates whose source is in that scope; `git commit -- :/<path>` (CWD-independent).
- **Backward-compat hole:** "no session record (raw `git add` / fresh session)" → legacy
  **whole-index** behavior. So a naive/unattended `git add -A` **bypasses** the scoping and
  reintroduces the incident's blast radius.

**Forged directive (from the same incident):** *Unexpected-gate-is-information* — a drift-abort /
ack-block is information about an **unmodeled state** (concurrent session, pending side-effect)
→ investigate before acting, never reflex-clear.

**Recurring recognition (surfaced by `mannaminne`, multiple sessions):** *"no tool solves
concurrent AI sessions."*

**Implication for this design:** committing drift safely requires knowing *which session
produced it and why*. An unattended nightly job structurally lacks that context and can bundle
another session's in-flight work. Therefore the steward runs **in-session, right after the drift,
with Claude's full global-CLAUDE.md context and Fredrik reachable** — never as a blind cron.
The nightly `nit update` may **detect + notify** ("drift accrued, resolve next session") but
must not auto-commit context-blind.

---

## 3. The five current `forward_only` files — audit + verdict

Measured 2026-07-11 (structure, churn, machine-specificity; secret *values* never read — only
markers counted).

| file | size / churn | machine-specific? | secrets? | true nature | verdict |
|---|---|---|---|---|---|
| `Documents/superwhisper/settings/settings.json` | 6 KB, 3 commits over months | **none** (0 /Users, 0 host) | **none** | pure human config: `favoriteModelIDs, modeKeys, replacements, vocabulary` | **MIS-FILED (by Claude this session). → plain-track (commit + push).** |
| `.claude/decisions_state.json` | 5 KB | none markers, but per-machine detection state | none | CC decision-detection scratch state | genuine runtime state → forward_only ok, or gitignore |
| `.claude/decisions_graphiti_cache.jsonl` | 86 KB | **47** /Users paths, **15** host refs | **7 secret-ish** | a cache | keep OUT of shared git → **local-only history**; never push |
| `.config/spela/config.toml` | 1 KB, single-host (Darwin) | 1 host marker | none | tiny mixed config + runtime-written | marginal — templatize flags or leave |
| `.codex/config.toml` | 6.5 KB | 15 `[projects."/Users/…"]` (identical across his Macs) | **8 secret-ish** (MCP bearer tokens) | mixed: human config + Codex-auto-appended project-trust + tokens | **templatize** stable part; secrets → env (partly done, `feat(codex): bearer-token extraction to env var`); local churn stays. Secret-care. |

**Two corrections Claude owns from this session (docs-not-lying):**
1. **superwhisper is NOT "noisy."** Its one real churn in history was Fredrik *adding a favorite
   model* (`"s1-vocab-v2-160MB"`) — a meaningful config edit. It "drifted" only because an edit
   sat uncommitted. Claude's earlier "cosmetic app-rewrite" characterization was an assumption
   the evidence refuted. It's a clean config that wants plain-tracking.
2. Claude earlier attributed the 09:53 TCC prompt to the nightly agent — that was **inference**,
   not proof; the trigger is genuinely unpinnable (any nit `git status` over `$HOME` touches
   Documents).

---

## 4. The full storage taxonomy (5 buckets)

| bucket | who writes it | reaches git + fleet? |
|---|---|---|
| plain tracked (~94% of dotfiles) | you | **yes** — commit + push |
| template `.tmpl` | you (machine renders locals) | **yes** — the template |
| secret `.age` | you (encrypted) | **yes** — encrypted |
| **forward_only** (today) | a machine/app, per-machine | **no** — frozen baseline + local restic snapshot |
| gitignored | ephemeral | no — untracked |

The mis-file risk: a config you tune that *also* gets app-rewritten looks like runtime-state
(it drifts) but belongs in the first group. `forward_only` currently **lumps together two
different needs** — runtime *junk* (don't want per-change git history) and drifting *config*
(you DO want the audit trail + sync). Freezing served the first and betrayed the second.

---

## 5. The two genuine constraints (why "just commit everything" isn't automatic)

Fredrik's premise — *"never minded it being a part of my git/nit history"* — is correct. You
*can* have git history for these. Only two things genuinely block a naive plain-commit, and
neither requires exile from git:

1. **Cross-machine merge conflict** — real *only* for genuinely per-machine-divergent files
   (the decisions caches: each machine's CC detects different things). One shared branch can't
   hold machine A's *and* B's version of one path without conflicting on every pull. **Fix:
   per-machine namespacing** (each machine commits to its own ref/path) — keeps full git
   history, just scoped per machine. NOT "exile to a local snapshot."
2. **Push of secrets/PII** — the graphiti cache carries machine paths + secret-ish lines.
   Committing it **locally** (audit trail) is fine; *pushing* violates the never-push-secrets
   invariant. **Fix: local-only history** for the secret-bearing ones.
3. (Bloat — real but minor; Fredrik doesn't mind it, and the steward batches/summarizes commits
   rather than one-per-append.)

So the "restic snapshot instead of git" was the *simple* way to dodge both constraints — at the
cost of the audit trail Fredrik actually values. Per-machine + local-only git history recovers
it.

## 5.5 Founding intent — forward_only was MEANT to reach git (primary-source archaeology, 2026-07-12)

Verified straight from git + the founding spec, in answer to Fredrik's question "did the original
design at least intend for the contents to reach git *eventually*?" **Yes — more strongly than §1
conveyed. The frozen-baseline outcome is a DOUBLE regression from founding intent, forced by one
false assumption.**

**The founding intent (`MIGRATION_CHECKLIST.md:653`, Apr 20 2026 — the v2 spec, predating any code):**
> "**Flush**: a user-invoked (or watcher-driven) `nit sync` copies target → source, **commits
> (optionally pushes)**. Conflicts can't happen because target is always ahead."

So the founding conception: runtime files drift dirty-by-design *between* syncs, then `nit sync`
flushes them **into git — committed, and originally even pushable.** Reaching git was the point.

**Then it eroded in two retreats:**

| state | `nit sync` did | reaches git? |
|---|---|---|
| **founding spec** (Apr 20) | copy target→source, commit, *optionally push* | yes — even fleet-wide |
| **`3d6bfec`** (May 17, first impl) | `git commit` of the paths, **not pushed** (`vec!["commit","-m","sync: forward-only runtime snapshot","--"]`) | yes — local history |
| **`cd1d18a`** (May 18, the reversal) | restic **snapshot** to `~/.local/share/nit/forward-only/`, no commit | **no — never reaches git** |

**The single load-bearing assumption that failed:** *"Conflicts can't happen because target is
always ahead."* True for a single-writer append-only file; false the instant the same tracked
path **diverges across machines** (each host's `nit sync` pushes a different "ahead" version →
collide on every fleet pull). `cd1d18a` hit exactly that and retreated all the way to local-snapshot.

**Why this settles the redesign direction:** the failed premise breaks *only* for the
per-machine-divergent subset (the caches). It never breaks for identical-across-machines config
(superwhisper). The taxonomy splits cleanly along that fault line:
- **Non-divergent** (superwhisper) → "always ahead" holds fleet-wide → the *original push intent
  already works* → plain-track + push. §9's superwhisper action is not new behavior; it's the
  founding intent un-regressed.
- **Divergent** (caches) → per-machine namespacing *restores single-writer-per-path*, making
  "always ahead" true again *per namespace* → reaches git with no conflict.

**So the Drift Steward is NOT new scope — it restores the founding forward_only intent** (runtime→git
flush) that two workarounds eroded, by fixing the root assumption (one writer per path) instead of
dodging it (exile to local snapshot). Founding note bearing on the trigger question: the spec
(`MIGRATION_CHECKLIST.md:658`) already contemplated automation — "a fswatch-style watcher, debounced
~1 min, runs `nit sync` whenever any forward-only target changes." Founding automation was a **dumb
deterministic watcher**, not Claude. The Steward's in-session hook is justified *only* by the parts a
watcher can't do (meaningful-vs-cosmetic classification, per-machine routing); the flush itself could
still be a watcher.

---

## 6. Proposed design — the Drift Steward

### 6.1 Principle
Delegate drift-triage to Claude, **in-session, right after the drift, with full context**, so
every config change lands in git/nit history with an honest message and near-zero cognitive load
on Fredrik — surfacing only genuine ambiguity.

### 6.2 Per-file routing (the criteria)
On a drifted tracked file, classify and act:

- **Meaningful config change** (new vocab, new hotkey, edited setting) → **commit** with a
  descriptive message; **push** if the file is non-divergent + secret-free.
- **Pure runtime append / per-machine state** (a cache growing, decision-state) → commit to
  **per-machine, local-only history** (Fredrik's stated preference: keep it in history, don't
  push). Only *skip* if genuinely valueless — Fredrik's call, default to keep.
- **Secret / PII detected** → **local-only**, never push; flag. (The pre-commit hook is the
  deterministic gate.)
- **Per-machine-divergent value** → per-machine ref/path (namespacing) or template.
- **Ambiguous** → surface to Fredrik with the specific question.

### 6.3 Timing + who
- **In-session, right after drift** — never a blind nightly job (§2). The nightly `nit update`
  may *detect + notify* ("drift accrued; resolve next session") but not auto-commit.
- **Claude specifically** — it holds the global-CLAUDE.md context and can surface questions;
  "risky to do by anyone else than Claude and me present to defer to if any true uncertainty
  arises" (Fredrik, verbatim).

### 6.4 Architecture (arsenal-and-harness)
- **Deterministic gates** (the arsenal): the pre-commit secret-scan hook; a machine-marker grep
  (`/Users/`, hostnames); an unchanged-content hash to skip no-ops.
- **LLM (the harness)**: the fuzzy calls only — meaningful-vs-cosmetic, message-writing,
  ambiguity-surfacing.
- **Composes with existing directives** (does not redefine):
  - *Autonomous-commit discipline* (global CLAUDE.md, Git Workflow Protocol) already mandates
    the core: "Claude meticulously reviews what would be committed, trusts its judgment, and
    commits — no per-file approval theater. Triggers: accumulated work-tree drift (… nightly
    `nit update` aborting on drift)… Asking is the exception, not the default." Its gates map 1:1
    (credential-scan → secret block; broken-state scan → don't commit garbage; honest message;
    "push? NEVER without explicit ask").
  - **The net-new deltas** over that directive: (a) a *scheduling/trigger* mechanism so it fires
    on drift, not only when a session happens to reconcile; (b) a *cosmetic-vs-meaningful* gate
    (the directive assumes drift = real work); (c) *per-machine routing*.

### 6.5 Per-machine local git-history mechanism — options to weigh
1. **Namespaced local ref** (e.g. `refs/nit/runtime/<host>`) that nit commits to and never
   pushes. Clean; full git history; no cross-machine conflict. Most faithful to the intent.
2. **A local git repo inside the snapshot dir** (`~/.local/share/nit/forward-only/` becomes a
   tiny repo). Simple, fully isolated from the main bare repo. Loses unified history with the
   dotfiles.
3. **`skip-worktree` + explicit capture** (see §7) — keeps them in the main repo but invisible
   to `status`/`add -A`; captured only on a conscious steward commit. Closes the raw-`add -A`
   loophole + the TCC stat, but overrides the recorded "detect all drift" invariant (§7).

Recommendation to evaluate first: **Option 1** (namespaced local ref) for the per-machine
caches, **plain-track** for the non-divergent secret-free configs (superwhisper), **template**
for the mixed-with-secrets ones (codex).

---

## 7. The `skip-worktree` finding (verified empirically 2026-07-11)

`git update-index --skip-worktree <file>` makes a file:
- show **clean** in `git status` even after a runtime rewrite,
- **immune to `git add -A`** (a bulk reconcile can't sweep it in — proven in a temp repo),
- captured into git **only** on a conscious `--no-skip-worktree` + `add` (i.e. an explicit
  steward/`nit sync` step).

It would close **both** residual gaps (the raw-`add -A` loophole **and** the TCC/Documents stat,
since `status` no longer reads the file). **But** it directly overrides the recorded
`3d6bfec` decision that `detect_pre_pull_drift` must keep *seeing* all drift (the safety
invariant that stopped the 2026-05-04 clobber). It is *probably* safe now — `cd1d18a` made
origin static, so these files never receive an incoming change to clobber — but that is a
deliberate reconciliation to make, not a free win. Treat as a candidate mechanism, not a given.

---

## 8. TCC / Documents angle (the thread that started this)

- `forward_only` files are still **stat-ed** by `git status` (detection unfiltered), so a file
  under `~/Documents` (SuperWhisper) makes the nightly `nit update` touch Documents → a TCC
  prompt if `nit` lacks the grant.
- Fredrik granted **Documents-only** (least privilege; nit tracks exactly one protected-folder
  file). `nit` is **ad-hoc-signed**, so TCC keys the grant to the binary's code-hash → a nit
  **rebuild** (`rebuild-nit`) re-prompts. The prompt **queues** (never wakes you at 3am).
- Reclassifying superwhisper to **plain-track** does NOT remove the stat (still tracked). Only
  `skip-worktree` (§7), or moving the file out of Documents, or dropping the grant-need entirely,
  removes it. Given it's one file + a queued (non-waking) rebuild-prompt, "leave the Documents
  grant + accept the rare queued re-prompt" is an acceptable resting state; `skip-worktree` is
  the clean elimination if the §7 invariant reconciliation is made.
- Full TCC reasoning (why nit-the-binary is the right grantee, why not FDA, cdhash-pinning as a
  feature) is in the sarpetorp session transcript 2026-07-11; the reusable rule already lives in
  global CLAUDE.md § Shell Command Robustness (TCC "Responsible Process" model) + § directive
  "TCC dialog attribution."

---

## 9. Immediate low-risk actions (independent of the big redesign — pending Fredrik's go)

1. **Revert `superwhisper/settings.json` to plain-track** + commit the current version (removes
   the mis-file; your vocab/replacements sync + provision again). It was added to `forward_only`
   in `fleet.toml` (nit commit `16104d82`, 2026-07-11) purely to silence the drift-abort — that
   was mitigation-as-deferral; back it out.
2. **Leave** `decisions_state.json` + `decisions_graphiti_cache.jsonl` as `forward_only` (genuine
   runtime state; the cache also has secret-ish content → must not push).
3. **codex/config.toml**: schedule a careful templatization pass (has 8 secret-ish MCP tokens —
   not a quick job).
4. **spela/config.toml**: leave (single-host, tiny) unless templatizing anyway.

---

## 9b. Build guidance (for the nit session — nit is RED-GREEN + fleet-rolled)

- **The steward MUST commit via session-intent-scoped `nit commit` (explicit paths), NEVER a raw
  `git add -A`.** This is the load-bearing constraint: `02c30db` scopes `nit commit` to this
  session's recorded staging, but the legacy whole-index path (raw `git add`) bypasses it and
  reintroduces the `3cf94eb8` blast radius (bundling + deploying another session's in-flight
  work). Dogfooded 2026-07-11: this doc's own companion commits used `nit add <explicit path>`
  precisely because the working tree carried other sessions' drift (`decisions_*`, `codex`,
  `karabiner`, hooks) at the time.
- **Trigger mechanism** = in-session, not cron. ⚠ **Correction to an earlier draft of this line
  (verified against the hook source 2026-07-12):** the existing **`fleet-drift-nudge`** hook is
  the SHAPE precedent, **not a ready seam.** What it actually does: it scans *my previous
  assistant turn's Bash commands* for a fleet-*mutation* (ssh-sudo-write / `hemma apply` /
  config-curl against darwin|shannon|turing) and, if found, nudges me to mirror that host's
  **hemma overlay.** It is stateless — no `git status`, no `nit`, does not read `last-sync.json`,
  does not know `forward_only` exists. Its trigger is "Claude just changed a remote host"; the
  Steward's trigger is "an app rewrote a local config while Claude was idle" — which has *no
  preceding Claude command to match*, so this hook structurally never fires for it. The
  **net-new** piece is a sibling hook (`nit-drift-steward-nudge`), a ~90% clone of the *shape*
  (UserPromptSubmit, conditional-silent, `additionalContext`, non-blocking) with a different
  *body*: instead of grepping my last turn, it reads durable local-drift state (`nit status`
  scoped to the declared set, or `last-sync.json` `drift_files`) and surfaces it at the next turn.
  The nightly `nit update` stays **detect + notify only** (it already writes
  `~/.local/share/nit/last-sync.json` with `drift_files` — the notify + the sibling hook both read
  that).

  **Three-role model (the trigger reconciliation — decision #1):** Claude is *never* triggered
  from outside. Drift is durable in the working tree (Hold, no trigger needed); deterministic
  detect+notify can run anytime incl. cron *because it never commits* (Detect); the sibling hook
  surfaces the durable drift into the next live session where Claude acts (Act). What makes
  in-session auto-action safe (the thing that killed "blind cron", §2): the Steward's auto-domain
  is bounded to the **declared app-owned drift set** — no Claude session hand-edits those files,
  so committing them can't clobber another session's in-flight work (the 3cf94eb8 hazard was
  human-authored CLAUDE.md/skills/docs). Anything drifting *outside* the declared set → surfaced,
  never auto-committed.
- **Rollout / activation** follows the existing `forward_only` pattern: change `fleet.toml`
  (+ any new `[sync]` keys), bump `.nit-version` (currently `b926b58…`, `nit 0.1.0`) + commit,
  and the `rebuild-nit` trigger recompiles across the fleet on next `nit update`. Ad-hoc-signed
  → each machine re-prompts any TCC grant after the rebuild (§8).
- **Test targets (RED-GREEN, pin these behaviors):**
  1. a `forward_only`/runtime file's **meaningful** edit gets committed (not skipped);
  2. a **cosmetic-only** rewrite (reordered JSON, bumped timestamp) is skipped/squashed;
  3. a **secret-bearing** runtime file is committed **locally**, **never pushed** (the pre-commit
     hook is the gate);
  4. a **per-machine-divergent** file commits to its namespaced ref with **no cross-machine
     conflict** on another machine's pull (the headline regression, mirroring `02c30db`'s
     `scopes_out_concurrent_session_index_entries`);
  5. the steward **never** runs a whole-index `add -A` (assert scoped commit).
- **superwhisper is multi-machine** (Mac Mini + MERIAN): plain-track **+ push** means it *syncs*
  the same vocab/replacements across both and provisions a fresh machine to the latest — the
  intended behavior, and the concrete payoff of un-freezing it.

---

## 10. Open decisions for Fredrik (to resolve in the nit repo)

1. Nightly = **detect + notify only** (safe), with the steward doing the actual commits
   in-session? Confirm this is the intended split.
2. Per-machine history mechanism: **namespaced local ref** (§6.5 option 1) vs local repo vs
   `skip-worktree`?
3. Pure-runtime caches: keep **local-only git history** (your stated preference) or gitignore?
4. `skip-worktree` adoption — worth reconciling against the `3d6bfec` "detect all drift"
   invariant to also kill the TCC stat + the raw-`add -A` loophole?
5. codex templatization — now or deferred?

---

## Appendix — evidence index (hashes + sources)

- `3d6bfec` — forward-only sync (original spec; detect-all-then-filter; nit sync = commit).
- `cd1d18a` — nit sync → local snapshot, not commit (the "frozen baseline" decision + its reason).
- `02c30db` — session-intent scoping (the concurrent-session keystone).
- `3cf94eb8` — the 2026-05-17 incident (bulk commit bundled + deployed another session's work).
- `cb0d179a` — "wip: accumulated drift from other sessions" (the bulk reconcile that swept in
  superwhisper before it was forward_only — the raw-`add -A` loophole in action).
- `a0373d23` → `cb0d179a` superwhisper diff — the meaningful `s1-vocab-v2-160MB` edit.
- `16104d82` — this session's `fleet.toml` forward_only add for superwhisper (to be reverted).
- Source: `nit/src/{config.rs,syncbase.rs,main.rs}`; snapshot dir `~/.local/share/nit/forward-only/`.
- Directives (global CLAUDE.md): *Autonomous-commit discipline*, *Unexpected-gate-is-information*,
  *Deterministic infrastructure*, *Arsenal-and-harness*, *Never push plaintext secrets*,
  *Search the personal corpus semantically* (forged this session).

---

## 10. The drift deadlock — a bucket this design did not have (found 2026-09-03 on MERIAN)

*Added while triaging real drift as test data for the Steward's classifier, per the
fixture rule. The real data disagreed with the imagined data, which is the whole
reason to look at it first.*

**What MERIAN looked like:** 18 modified tracked files plus one deleted, its nightly
`nit update` aborting on drift every night, 536 commits behind, HEAD dated 2026-07-29.
Read as ordinary drift, it looks like five weeks of unreconciled local work on a
travel machine — exactly the case the Steward exists to triage.

**It was nothing of the kind.** Measured, every one of the 19 files carried the
*identical* mtime — `2026-06-28 03:00:02`, the nightly sync hour, to the second — and
every one's exact current content already existed in the object store. Not one unique
byte. The reflog shows the last successful `pull: Fast-forward` on **2026-06-27**;
after it, nothing but a single local commit on 2026-07-29. `launchctl list` reports the
nightly agent present with last exit status **1**.

**So the failure is a deadlock, and it is self-sustaining:**

> The drift-abort refuses to pull while tracked files are dirty. Those files can only
> become clean by pulling. Once a machine enters this state it can never leave it on
> its own, and every subsequent night re-proves the same abort.

**The safety invariant worked perfectly and that is the problem.** `detect_pre_pull_drift`
did precisely its job for **67 consecutive nights**, protecting a worktree whose contents
were already safe in git. It was never wrong; it was never *heard*. The abort writes
`last-sync.json` and stops. Nothing surfaces to Fredrik, nothing surfaces to a session,
and a travel machine drifts 536 commits behind in silence.

### 10.1 Why the Steward as designed would have got this wrong

§6.2 routes a drifted file by asking whether the change is *meaningful config* or *pure
runtime append*. These 19 are neither. They are **stale** — a frozen worktree from a
single night, older than the HEAD they are compared against. Classified by content alone,
`presence_service.py` or `AdGuardHome.yaml` reads as a meaningful config change and would
have been committed, resurrecting June content over five weeks of newer work on top.

**The missing question is cheap, deterministic, and belongs in the arsenal, not the
harness:** before classifying a drifted file at all, ask *does this exact content already
exist in git history?* One `hash-object` plus one `cat-file -e`. Content already in
history is by definition not new work, whatever it looks like. This gate runs BEFORE the
meaningful-vs-cosmetic gate and short-circuits it.

A second deterministic signal, nearly free: **identical mtimes across many drifted files**
means one process wrote them all at once, which is a machine event rather than human
editing. Nineteen files sharing a timestamp to the second is not eighteen decisions.

### 10.2 What this adds to the build list

1. **A staleness gate** (deterministic, above) ahead of the classifier.
2. **A deadlock detector**: drift + behind-by-N + last-successful-pull older than a few
   days is a distinct condition from ordinary drift and deserves its own name and its own
   message, because the remedy is opposite — *discard and pull*, never *commit*.
3. **Someone has to be told.** The nightly may not auto-commit (§2, and that holds), but
   an abort that repeats for a week is not a routine outcome; it is an incident. The
   cheapest honest fix is for a repeated abort to reach Fredrik the way anything else on
   this fleet reaches him — ntfy — and for a session to surface it on first contact.
   **Detect-and-notify was already sanctioned in §6.3; it was simply never built, and the
   67 nights are what that costs.**

**Generic lesson worth carrying beyond nit:** a guard that fires correctly, repeatedly,
and silently is indistinguishable from a guard that never fires. *Make failures visible
before fixable* covers the diagnosis; this is its scheduling half — **a recurring
successful refusal needs an escalation path, or the protection it provides becomes the
outage it was meant to prevent.**

### 10.3 Built 2026-09-03 — and what building it found

Shipped in `src/drift_triage.rs` (248 tests green, clippy and fmt clean):
the staleness gate, the deadlock detector, and the ntfy escalation. Wired into
the `nit update` abort message (per-file verdicts) and into `nit status` (a
deadlock line under the calm one-liner).

**Three defects surfaced only in end-to-end use. All eighteen unit tests passed
through every one of them.**

1. **Both git calls were cwd-sensitive.** `hash-object` resolves its argument
   against the PROCESS directory regardless of `--work-tree`, and a bare
   pathspec is likewise cwd-relative. Run from anywhere but `$HOME` — which is
   to say, run by launchd — every file came back `Unknown`. A broken answer
   wearing a cautious one's clothes. Fixed with an absolute path and the `:/`
   root-relative pathspec that `cmd_commit` already uses for this exact reason.
2. **`--diff-filter=d` returned ZERO commits** for a path with fourteen. It
   looked like the tidy way to skip commits that deleted the path; it needs
   diff generation that `git log` does not perform under default history
   simplification. Replaced by tolerating an unresolvable rev per commit.
3. **The alert could not be delivered at all.** ntfy returned 403: the fleet's
   `NTFY_TOKEN` belongs to the `router` user, whose grants are
   `shannon-security*`, `router-security*`, `dinmamma-watch` and `mannaminne`.
   A new topic is not covered. Compounding it, the token line in the tier files
   is `export NTFY_TOKEN=…` and the parser only matched the bare form.

**The third one is worth dwelling on.** A feature built to end silent failure
was itself failing silently, in the same shape, one layer up: the alert did not
arrive, and nothing said so. The code now prints a line when delivery fails.
**Every alerting path needs a report-on-failure, or it inherits the very
disease it was prescribed for.**

Standing decision for Fredrik: the ntfy ACL needs one grant before the push can
work. Until then the local surfacing (abort message plus `nit status`) is live
and the push is a no-op that announces itself.

### 10.4 Still open

- **The machine with the problem is the one least able to report it.** MERIAN
  is asleep most nights and often off the VPN. A push fired from the deadlocked
  machine is the cheap 80%, not the whole answer; the complete version is a
  fleet-health check on Darwin, which is always on, reading each machine's
  `last-sync.json` and noticing a machine that has gone quiet as well as one
  that is failing. Deliberately not built here — it is a different and larger
  design than §10 asked for.
- **`nit` still has no canonical discard.** The abort message's first suggested
  remedy is a raw `--git-dir`/`--work-tree` escape hatch, which is exactly the
  shape *Aesthetic-as-decision* flags. The staleness gate now makes a safe
  version possible — discard only what is PROVEN recoverable from a named
  commit — but the shape of that command is a design fork on shared tooling,
  so it is proposed rather than assumed.

## 11. Should the Steward also own hemma overlay drift? (Fredrik, 2026-09-05)

**First, a correction to something said earlier the same day.** nit drift and
hemma overlay drift were described as "different surfaces with no overlap".
Measured: **692 files under `dotfiles/system/` are nit-tracked**, 126 of them
Shannon's. So they are not two surfaces. They are **two layers of one pipeline**:

| layer | compares | watched by |
|---|---|---|
| **upper** | the overlay SOURCE in `dotfiles/system/<host>/` vs git | nit — so the Steward, already, for free |
| **lower** | the DEPLOYED copy on the machine vs that overlay source | `hemma system-diff`, via `shannon-drift-watch` |

The Steward therefore needs no extension at all to cover the upper layer. The
open question is only the lower one.

### 11.1 The judgment generalises; the plumbing does not

**Generalises cleanly:**
- The **staleness gate** works unchanged — overlay content lives in the same git
  history, so "do these exact bytes already exist?" is the same query.
- The **classification** is the identical question: meaningful edit, stale copy,
  runtime junk, or secret-bearing.
- The **escalation** is the same need, and is the strongest argument (§11.2).

**Does NOT generalise:**
- **Transport.** nit drift is local to the machine running the check. Overlay
  drift is remote: SSH, unreachable hosts, and "the machine is asleep" becomes a
  first-class state rather than an error. MERIAN was unreachable three separate
  times during one evening's work.
- **Authority direction.** For nit, either side may be right — a config you
  tuned is as likely correct as the repo. For a deployed system file the overlay
  is normally the source of truth, so drift there usually means someone
  hand-edited `/etc` on the box. That is a different and more suspicious event,
  and it wants a different default.
- **Write path.** Capturing nit drift is `nit add` + commit. Capturing overlay
  drift is `hemma system-pull`, which carries interactive prompts — the exact
  surface a past session patched badly enough to earn the *Consult before
  shared-infrastructure design changes* directive.
- **Ownership.** Overlay files are root-owned on the target, so capture needs
  sudo, and *Push-pull operations must be symmetric* applies in full.

### 11.2 The shape this argues for: one classifier, two adapters

A source-agnostic core that takes `(path, current bytes, tracked bytes, machine)`
and returns a routing verdict, fed by two adapters: nit's local worktree, and
hemma's `system-diff` over SSH. `shannon-drift-watch` then becomes a thin
adapter instead of a parallel implementation.

**The prize is the ESCALATION, not the code reuse.** On 2026-09-05 five
independent silent breaks were fixed in the hemma-side alert while a fresh
alerting path was separately built for the nit side — two hand-rolled
escalations, each free to rot alone, and the hemma one had been incapable of
delivering anything for months without a single symptom. One path, kept honest
in one place, is what prevents a third instance. This is §10.3's lesson applied
one level up: *an alerting path without a report-on-failure inherits the disease
it was prescribed for* — and the way to keep that honest is to have ONE of them.

### 11.3 What this does NOT make obsolete

The **liveness prober** is untouched by any of this. It answers "is Shannon
alive", not "does its config match" — and Shannon runs the bedroom lights and
Home Assistant, where a dead host has woken Fredrik at night. Keep it, and it is
a third candidate for the same unified escalation path.
