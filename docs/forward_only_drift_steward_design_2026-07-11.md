# forward_only redesign + the Drift Steward — design doc

**Date:** 2026-07-11
**Status:** BUILT 2026-10-15 (§ 16). The sections before § 16 are the design as it was argued; § 10.4 holds the final rulings.
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

## 10. Decisions — two RULED 2026-09-12, five still open

**Superseded**: this section previously listed five undifferentiated open questions. Two are now
settled by Fredrik, and the remaining set changed shape once the prior art in §15 was read.

### 10.1 Ruled by Fredrik (2026-09-12)

- **BUILD IT.** The founding spec (`MIGRATION_CHECKLIST.md:789`) gated this work on friction:
  *"when manual edit-source flow becomes painful in practice… worth measuring real-world friction
  before adding complexity."* Fredrik's ruling: *"The friction as build trigger has in my mind
  proven itself to exist many many times over the months since I built nit. Let's do this."*
  The gate is discharged. It is not an open question any more and should not be re-asked.
- **The intelligence lives in a Claude session, not inside nit.** *"I think putting the
  intelligence in a Claude session seems wise."* This settles the fork §15 surfaces: the 2026-04
  deferred design proposed piping the fuzzy work to an LLM through `fabric`, to keep nit
  provider-agnostic. That stays true of nit — nit gets NO LLM call, no HTTP client, no fabric
  dependency — but the fuzzy half now happens in a live Claude session rather than inside a nit
  subcommand. nit's job is the deterministic half plus a queue the session reads.

### 10.2 Still open — with a recommendation on each

1. **The per-machine caches: namespaced local ref, or just gitignore them?**
   *Recommendation: gitignore.* `DOTFILES_STRATEGY.md` decision #8 already ruled that
   "sessions/logs/caches are machine-specific or ephemeral" and stay ignored; the decisions caches
   were tracked anyway, which is the departure that created this whole problem. The audit trail
   for a machine-written cache is close to worthless — the decisions themselves live in
   `DECISIONS.md` and Graphiti, and the cache is derived from them. Choosing this **deletes the
   entire per-machine-namespacing mechanism from the build**, which is the largest simplification
   available, and it removes the only part of the design that cuts against the fleet's stated
   "mirror everything everywhere" principle (§14). If a cache is ever the ONLY copy of something,
   that is a bug in the cache's owner, not a problem for nit to solve.

   **Checked 2026-10-10, and the premise above is only half right.** `decisions_state.json` is
   a rolling list of the last 20 decision titles, written by `decision_logger.py` beside the log
   (not rebuilt from it, though everything in it is also in the log). `decisions_graphiti_cache.jsonl`
   is NOT derived. `decision_logger.py` appends each decision to it first-hand (title, decision,
   rationale; 54 on the Mac Mini, the last on 2026-08-31), and two things read it: `/recall`
   (its list of recent decisions) and the search for an older decision that a new one replaces.
   The same decision also goes to the project's `DECISIONS.md`, and to Graphiti only when its
   paid model has credit, which it often has not. So ignoring this file would leave a rebuilt
   machine with an empty recent-decisions list and no supersession search, and for decisions
   made while Graphiti was dry it is one of two copies. It is an append-only log, one line per
   decision, which suggests a third option this section did not weigh: keep it tracked and let
   git join two machines' appended lines by itself (`merge=union` for that one path).
   Put to Fredrik 2026-10-10; not ruled.

   **Fredrik, 2026-10-15:** why not simply commit both every time, and hand any conflict to a
   session through the drift queue? Weighed: the only real harms of auto-committing are (a) a
   conflict met by the unattended 03:00 pull stops that machine's whole sync until resolved, and
   (b) the state file is rewritten whole, so two machines that both log decisions conflict on it
   every time, not rarely. Both have answers that keep his proposal: the log gets `merge=union`
   (append-only lines join without conflict), the state file gets a merge driver that rebuilds it
   from the merged log (its last 20 entries), and whatever still conflicts goes to the drift
   queue for a session to resolve. **Recommended: track and auto-commit both on that basis.**
   Awaiting his yes.

   **Details checked the same day (2026-10-15), which change the recommendation's shape:**
   - **The log is not purely append-only.** `supersede_decision()` rewrites the old decision's
     line in place (adds `superseded_by`). It has never run (0 of 54 rows superseded), but under
     a union merge a rewritten line next to another machine's new line comes out twice. Fix:
     supersession appends its own line, and readers fold by `decision_id`.
   - **The titles file needs no merge driver; it can go.** A custom merge driver needs git config
     set on every machine (config is not versioned), while `merge=union` is built in. The titles
     file's two readers (`claude_decisions_cli.py` list/status, `user_prompt_detector.py`) go
     through `get_recent_decisions()`; point that at the log's last 20 rows and delete the file.
     One source, nothing to merge.
   - **Who commits: the writer, right after writing.** The decision logger knows exactly what it
     wrote, so no judgment is needed and the file is never dirty at 03:00. But `nit commit`
     commits everything THIS session staged, so a logger commit from inside a busy session would
     sweep that session's other staged work in. nit needs a path-limited form
     (`nit commit -m … -- <path>`). Small, but a change to shared tooling.
   - **Nothing pushes automatically.** `nit update` never pushes; the Mac sat 1 commit ahead on
     2026-10-15. Other machines see new decisions only after some session pushes. An automatic
     push from the logger would also publish any other unpushed local commit, so: no auto-push.
   - **No secrets.** trufflehog read the whole file (128,827 bytes, 0 findings); the July audit's
     "7 secret-ish" were word matches. The file has been on the private remote since April.
   - **One writer today.** All 54 rows carry `/Users/…` paths (Mac Mini). Darwin has the logger
     and the files but has never logged a decision; Shannon only has the copies.
   - **The switch is one commit:** remove both paths from `fleet.toml` `forward_only`, add
     `~/.gitattributes` (new tracked file at `$HOME` root, read only by the nit repo), and commit
     the live log, or the next nightly aborts on it.

### 10.4 Decisions still owed for the Steward as a whole (2026-10-15)

Found while checking the above. On 2026-10-15 the Mac's nightly had been deadlocked 3 days, and
none of the three blocking files was app-written: a glossary, `.hammerspoon/init.lua` and
`dotfiles/docs/connections.md`, each another session's uncommitted edit. The design already
covers such files: §2 puts the Steward in-session because of concurrent sessions, and §9b keeps
them outside the auto-commit set, surfaced and never committed blind. What it lacked was (a) a
way for the nightly to keep syncing while such edits sit there, and (b) the evidence the Steward
needs to judge them, namely which session made the edit and whether it is still open.
(Corrected 2026-10-15: a first draft of this paragraph said the design did not cover them.)

1. **Narrow the pre-pull abort to files the pull would touch.** Why it exists (`164513c3`): a
   pull over dirty files could merge badly. Git itself refuses to overwrite an uncommitted file
   that incoming commits change; files the pull does not touch are left alone. So: fetch, list
   files changed between HEAD and upstream, abort only if that list meets the dirty set. Keeps
   "never clobber" exactly, ends most deadlocks. Changes the sacred invariant, so his call.
   (Post-pull steps that write files — templates, secrets, triggers — are already drift-checked
   individually; confirm each before building.)
2. **A dead session's uncommitted edits.** The Steward must never commit a live session's work
   (`3cf94eb8`). When the session that made the edit has ended: commit as `wip:` after some
   hours, or only ever surface? His call.
3. **May the Steward push?** Private repo, non-secret files, but "push only on explicit ask"
   stands today. Without pushes the fleet does not converge.
4. **Nightly = detect + notify only** (10.2.2) — recommended yes, unchanged; needs his yes.
5. **A canonical `nit discard`** (§11.4) — discard only content proven present in a named
   commit. Shape is a shared-tooling choice.

**Ruled by Fredrik 2026-10-15:**
- 10.2.1 with the three changes above: *"All sound reasonable!"* Build it.
- Item 1 (narrow the abort): *"Sensible, go ahead."* Verified first in a scratch repo: a merge-mode
  pull refuses with "would be overwritten by merge" when incoming touches a dirty file, and
  leaves an untouched dirty file intact.
- Items 2 and 3 (dead sessions' edits, pushing) are NOT fixed rules. His words: *"Claude is the
  steward… should have some insight and be able to find out, in talking with me and inspecting
  the state of the session that made edits, whether the changes are unfinished or not ready to
  spread/push… what the prudent and wise course of action is."* So the Steward judges per case,
  and asks him when the evidence does not settle it. Consequence for the mechanical half: the
  queue must hand over that evidence. For each drifted file, `nit drift-triage` records the
  sessions whose transcripts wrote the path (Edit/Write `file_path` in
  `~/.claude/projects/**/*.jsonl`) and whether each is still running; a file with no writing
  session was edited by hand or by an app, and the Steward asks. Checked on the 2026-10-15
  blockers: all three trace to Deliberus sessions that day or to a hand edit.
- Item 4 is his recorded position already (*"completely acceptable if the nightly sync fails with
  a notification… I do not want to risk machine-local edits being clobbered"*): closed.

Decided without him (reversible, implementation-level): `nit discard` refuses unless the exact
bytes are in a named commit, and names that commit (item 5); `skip-worktree` stays out (10.2.3,
moot once the caches are committed); hemma adapter later (10.2.5); codex config is a separate
job (10.2.4).
2. **Nightly = detect + notify only.** *Recommendation: yes, unchanged.* Already Fredrik's
   recorded position, and the escalation half now exists (the `fleet` ntfy topic, 2026-09-05), so
   the nightly can refuse loudly instead of silently.
3. **`skip-worktree` adoption?** *Recommendation: no.* It overrides the `3d6bfec` "detect all
   drift" invariant that stopped a real 2026-05-04 clobber, and if decision 1 goes to gitignore,
   the problem `skip-worktree` was solving disappears with it.
4. **codex/config.toml templatization** — still deferred; it is a separate careful job (8
   secret-ish MCP tokens), not part of the Steward.
5. **The hemma-overlay adapter (§12) — build now or later?** *Recommendation: later.* Ship the nit
   adapter first; keep the classifier's interface able to accept a second adapter without a rewrite.

### 10.3 A fourth verdict the classifier needs (added 2026-09-05)

The design's verdicts were commit / commit-locally / skip / ask. A fifth kind of drift exists and
resolves differently: **a deploy TARGET tracked as if it were a SOURCE.** `~/.local/bin/macmini` on
MERIAN was one — a file a trigger writes, tracked as though a human authored it, so it drifts every
time the trigger runs and no commit can ever settle it. **The fix is to UNTRACK, not to commit.**
This is mechanically detectable (the path is written by a declared trigger) and the April notes
predicted more of them: *"Other similar vestiges would show up as persistent nit-update drift
aborts and resolve the same way."* A sweep for the rest is owed.

---

## 11. The drift deadlock — a bucket this design did not have (found 2026-09-03 on MERIAN)

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

### 11.1 Why the Steward as designed would have got this wrong

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

### 11.2 What this adds to the build list

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

### 11.3 Built 2026-09-03 — and what building it found

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

### 11.4 Still open

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

## 12. Should the Steward also own hemma overlay drift? (Fredrik, 2026-09-05)

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

### 12.1 The judgment generalises; the plumbing does not

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

### 12.2 The shape this argues for: one classifier, two adapters

A source-agnostic core that takes `(path, current bytes, tracked bytes, machine)`
and returns a routing verdict, fed by two adapters: nit's local worktree, and
hemma's `system-diff` over SSH. `shannon-drift-watch` then becomes a thin
adapter instead of a parallel implementation.

**The prize is the ESCALATION, not the code reuse.** On 2026-09-05 five
independent silent breaks were fixed in the hemma-side alert while a fresh
alerting path was separately built for the nit side — two hand-rolled
escalations, each free to rot alone, and the hemma one had been incapable of
delivering anything for months without a single symptom. One path, kept honest
in one place, is what prevents a third instance. This is §11.3's lesson applied
one level up: *an alerting path without a report-on-failure inherits the disease
it was prescribed for* — and the way to keep that honest is to have ONE of them.

### 12.3 What this does NOT make obsolete

The **liveness prober** is untouched by any of this. It answers "is Shannon
alive", not "does its config match" — and Shannon runs the bedroom lights and
Home Assistant, where a dead host has woken Fredrik at night. Keep it, and it is
a third candidate for the same unified escalation path.

### 12.4 A third instance of the same disease, found 2026-09-05

Shannon's nightly sync had been dying for ~10 weeks and **reported a success the
whole time**. `nit update`'s secrets-drift abort returned an error WITHOUT
writing `last-sync.json`, so the file kept saying `ok — 2026-06-27` while the job
failed every night. Fixed in nit (`SyncResult::SecretsDrift` + a record at the
abort + `nit status` flagging any non-ok result).

**Why it belongs in THIS doc rather than only in nit's changelog:** it is the
third distinct way the fleet's health reporting lied inside three days, and the
Steward inherits all three.

| # | what happened | why nothing noticed |
|---|---|---|
| 1 | MERIAN's pre-pull abort, 67 nights | correct refusal, no escalation path |
| 2 | shannon-drift-watch, months | correct detection, five broken alert layers |
| 3 | Shannon's secrets abort, ~10 weeks | **no record written at all — the file reported the last SUCCESS** |

The third is the worst of the three and the least likely to be caught by
instinct: silence prompts eventual suspicion, a stale *success* does not. So the
Steward's escalation needs BOTH conditions, and they are not the same test:

- **refusal** — an abort happened, and here is its record;
- **silence** — no successful sync for N days *regardless of what the last record
  claims*, because the record may be months old and cheerful.

Design consequence: **never derive health from the last record's `result` alone.
Derive it from the AGE of the last SUCCESS.** A status file is a claim by a
process that may have died before writing.

Related, and settled 2026-09-05: fleet alerts now publish to a single `fleet`
ntfy topic rather than a topic per purpose. A new topic silently drops alerts
until someone subscribes on their phone, which is how `fleet-sync` spent its
first morning publishing into a void. The Steward should publish there too, and
should not invent its own.

---

## 13. The hook, concretely (2026-09-12)

Written down because "an in-session hook" was agreed in the abstract and never drawn.

The detection cannot be event-driven from inside Claude Code: app-caused drift happens with no
session running and no tool call to intercept. So the split is the three-role model of §9b, with
one concrete piece per role:

- **Hold** — the drift sits in the working tree. Durable, needs nothing.
- **Detect** — `nit drift-triage`, a nit subcommand. Enumerates drifted tracked files, runs the
  deterministic gates (the staleness check built 2026-09-03, the secret scan, the machine-marker
  grep, the deploy-target check of §10.3) and writes a queue to
  `~/.local/share/nit/drift-queue.json`. No LLM. Safe to run from cron **because it never commits**.
- **Act** — `drift-steward-nudge`, a `UserPromptSubmit` hook, a shape-clone of the six existing
  conditional-silent nudges. Reads that one JSON file; emits nothing when it is empty. When it is
  not: a few lines naming the count and the files needing a verdict, so Claude picks it up in the
  session with full context. A `--dismiss` path so it cannot nag.

Rejected alternatives, with reasons: **SessionStart** (fires on every unrelated session — a spela
evening should not open with a dotfiles detour); **a filesystem watcher** (the nightly `nit update`
and its abort already detect; a watcher adds a second always-on process for no new information).

---

## 14. Per-machine history cuts against a stated fleet principle

Recorded because it was raised as an objection and deserves to stay visible if decision 10.2.1 ever
goes the other way. `DOTFILES_STRATEGY.md § Per-Machine Scope` states the guiding principle as
cross-OS isomorphism — *"when in doubt, mirror it"* — and decision #11 quotes Fredrik directly:
*"the closer each environment automagically mirrors all other environments, the happier I will be.
The less surprises, the better."* A `refs/nit/runtime/<host>` namespace would be the first mechanism
in the fleet that deliberately institutionalises divergence. Defensible for machine-written caches,
but it should be adopted as a NAMED exception with that principle quoted, never as a default.

---

## 15. Prior art — what was already designed, and what others hit (2026-09-12)

### 15.1 In our own repo: `nit pick --apply` (MIGRATION_CHECKLIST.md:723, Apr 2026)

A deferred v2 design already exists for the closest neighbouring problem — promoting VALUABLE drift
back into a TEMPLATE source, where this design handles PLAIN files. It was never mentioned in §§1–12
and should have been. Its algorithm is worth reusing directly, and its core idea is the
**verification step**: apply the drift diff to the source, **re-render, and accept only if the
re-render reproduces the drifted file byte-for-byte**; otherwise revert and refuse. That converts a
plausible-looking merge into a checked one. It also documents its own limitation honestly — when
the drift lands inside one conditional branch, only that machine's branch is updated.

The sibling `nit pick --apply --with-llm` proposed piping source + rendered + drifted target through
`fabric` when the plain patch fails, explicitly *"keeps nit LLM-agnostic and reuses the existing
fabric infrastructure rather than embedding a Rust HTTP client per provider."* Fredrik's 10.1 ruling
supersedes the delivery mechanism while keeping the constraint: **nit itself stays LLM-free.**

### 15.2 Outside: three findings that shaped the design

- **chezmoi's `autoCommit` cannot see this drift at all.** Its own docs state it fires only when
  chezmoi already knows the SOURCE changed — after `chezmoi add`/`edit`/`import` — and not from
  general file modification in the home directory. That is precisely our gap, independently
  confirmed: the mature tool in this space does not solve app-written drift either.
- **Nobody has solved strip-secrets-before-auto-commit.** chezmoi issue #3745 asks for exactly the
  capability our secret-bearing verdict needs (a chance to templatize before an automatic commit)
  and it is open. Our answer — commit those locally and never push — is a legitimate design choice,
  not a shortcut around a solved problem.
- **etckeeper's central warning is the same hazard.** Its README's own caution about versioning
  `/etc` is that it creates copies of files that must stay secret (`/etc/shadow`). A decade-old tool
  doing daily automatic config commits names secret propagation as the principal risk, which is why
  the secret gate here is deterministic (the pre-commit hook) rather than a judgment call.
- **Why the LLM half belongs in a live session rather than a standalone tool**: an empirical study
  of LLM commit-message generation attributes **10.5% of underperformance to missing context** —
  the diff alone does not carry the *why*. A session that just watched the change happen has that
  context; a cron job invoking a model over a bare diff does not. This is the measured form of the
  argument §6.3 made from intuition. (arXiv:2502.18904)



## 16. Built 2026-10-15 — what exists, and the calls made without asking

**nit** (commit `bbb1274`):
- `src/pull_gate.rs` — the narrowed pre-pull gate (§ 10.4 item 1). Blocks on: an edit the
  incoming commits touch; anything staged when the pull is a true merge; and, when what is coming
  cannot be determined, any edit at all. Everything else is pulled around. Behaviour measured in
  scratch repos before writing it; pinned by unit tests and five real-git end-to-end tests
  (`tests/update_flow.rs`).
- `PulledAroundGuard` in `main.rs` — copies each pulled-around edit aside before the pull and
  compares afterwards; a changed one keeps its copy under `~/.local/share/nit/pulled-around/` and
  the run reports `edit_touched`. Triggers that watch an edited file are skipped for that run.
- `src/drift_queue.rs` — `nit drift-triage` and the queue file. Writer evidence comes from Claude
  Code transcripts (`~/.claude/projects/**/*.jsonl`) and running-session records
  (`~/.claude/sessions/<pid>.json`), searched from the path's last commit, at most 21 days back.
  An editing tool call is "edited"; any other tool input naming the path is "named". Codex
  transcripts (`~/.codex/sessions`) are read too since the same evening; see Later the same day.
- `nit commit -m … -- <paths>` commits only those of the session's staged paths; `nit discard`
  puts back HEAD's version only when nothing is lost (stale content already in history, or a
  deletion), never for staged or new content.

**dotfiles**: the decisions log committed by its writer and merged by union (§ 10.2.1, commit
`f34328fe`); the `drift-steward-nudge` hook; the `drift-steward` skill (the sorting rules of
§ 6.2, rewritten as judgement per his ruling rather than fixed routes).

**Calls made without asking, each reversible** (stated so they can be overruled):
1. **Who hears the nudge** (the old open question "any live session vs a designated one"): a
   session working in `~/dotfiles` or `~/Projects/nit` always; any other session only when the
   last nightly sync was blocked. One session at a time holds a two-hour claim; each session
   hears a given queue once. Reason: § 13's objection to interrupting unrelated work.
2. **A session's own edits are not shown to it**: that is its work in progress.
3. **The queue is refreshed** after every `nit update`, and by the hook in the background when it
   is over an hour old (one-shot process, ten-minute lock).
4. **`git pull --no-rebase --no-edit`** in `nit update`, explicit rather than relying on branch
   config, and so that a merge never opens an editor.
5. **Per-machine namespacing (§ 6.5) is not built.** With the decisions log merged by union and
   the titles file gone, no remaining tracked file needs it.

**Later the same day** (each found by using the steward on the real queue):
- **Spellings.** The writer search matched only absolute paths and found no session for two of
  the three blocking files; sessions write `~/…` and `$HOME/…` too. Fixed in `e5eecbec`.
- **"Own work" means edited, not named.** The hook first hid a file because only the steward
  session itself had named it in a shell command; only an edit by this session now counts.
- **Codex was invisible, then read.** `.hammerspoon/init.lua` and its untracked `spela.lua` came
  from a Codex session (Fredrik knew; nit could not see it). The queue now searches
  `~/.codex/sessions` too. Codex edits through Python and shell inside its `exec` tool, often
  building the path in pieces, so its touches are "named in Codex shell", or "edited? (Codex
  shell)" when that command visibly writes; whether a Codex thread is still open is unknown.
  A transcript reads with `codexdistill <id>`, the same program as `ccdistill` under a second
  name (its default store is Codex; either name reads either format). Subagent threads carry
  no request of their own; their task is in the parent's spawn call.
- **A directive plus a second hook trigger** (his request: make the skill load when a session
  elsewhere meets drift). Global CLAUDE.md, Nit section: committing, discarding or restoring a
  tracked file this session did not edit goes through the `drift-steward` skill, cued on nit's own
  output strings. The hook reads the previous turn's tool output in any session and names the
  skill and the never-rules when one of those strings appears. Every firing is logged to
  `~/.local/share/drift-steward/fired.jsonl`, so whether the skill really loads can be measured.
- **Reshaped 2026-10-17 from that log** (30 firings in three days, four led to work). nit's
  ordinary `Modified` status list is no longer a cue: 18 reminders on it led to nothing, since
  seeing another's file in a list is harmless. The hook reads only `nit`/`hemma` command output
  (reading its own source or log had set it off). The moment of risk moved to a new PreToolUse
  guard, `~/.claude/hooks/foreign-edit-guard`: once per command it denies staging, discarding,
  removing, restoring or stashing a changed tracked file this session did not write, naming whose
  edit it is from the queue; the same command again within 30 minutes goes through.
- **Rollout.** Pin `e5eecbec`, then `b6e3822e` (the Codex reader). A hand-installed nit is replaced by the pinned one at the next
  `nit update` (`rebuild-nit`), so testing a build means moving the pin. Shannon's aarch64 copy
  is staged on Darwin; `stage-nit-prebuilt.sh` had a check that refused correct builds (`grep -q`
  under `pipefail`), fixed.

---

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
