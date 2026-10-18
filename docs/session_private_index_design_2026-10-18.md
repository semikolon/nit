# A private staging area per session — design (2026-10-18)

**Status:** approved by Fredrik 2026-10-18 ("Go ahead"), built the same day: `src/session.rs`,
`tests/commit_flow.rs` (the two-session, compare-and-swap, PID-reuse, adoption and partial-file
cases run the built binary against a throwaway `$HOME`).

## Why

Every Claude Code (and Codex) session on a machine shares one git index, and nit tracked which
staged entries were whose in a side list (`~/.local/share/nit/staged/<anchor>.json`). Each git
command that touches the index without updating that list became its own bug: 23 add/commit
fixes between April and October 2026, 10 of them in October. The last three, met while
templating spela's config:

1. `nit commit -- <path>` recorded a template target's fresh render instead of its staged
   removal (the pathspec commit takes the work tree, and the commit deployed templates first).
2. A removal staged with `nit rm --cached` was not this session's, so `nit commit` saw nothing.
3. `nit commit --amend` rewrote another session's commit that had landed in between.

Earlier: the 2026-05-17 21-file bundle, the 2026-08-27 half-committed rename, the 2026-09-30
hunk staging that a pathspec commit overrode. The May intent was already "a commit takes exactly
what this session staged" (TODO.md, keystone); the September note asked for "commit honours the
index as staged". A private index is the direct form of both.

git supports it natively: `GIT_INDEX_FILE` points any git command at another index file.

## The model

- **Each session has its own index file**, `~/.local/share/nit/sessions/<anchor>.index`, with
  `<anchor>.json` beside it: `{ "base": <commit it was seeded from>, "started": <the anchor
  process's start time>, "last_commit": <the newest commit this session made> }`.
- **Seeded lazily** from HEAD (`read-tree HEAD`) by the first command that stages.
- **Staging commands write only there**: `nit add` (including template-source redirects),
  `nit rm`, `nit mv`, path-form `nit reset`, `nit restore`. For anything else,
  `GIT_INDEX_FILE=$(nit session-index) git apply --cached …`: the next nit command finds what
  was staged that way (the record keeps `built_from`, the HEAD the index was last rebuilt on) and
  makes it this session's. The shared index is not touched by staging.
- **Reading commands that show staging read there**: `nit diff --cached` / `--staged`,
  `nit status` (counts and lists this session's staged paths).
- **What this session staged** = its index compared with its base (`diff-index --cached <base>`).
  No side list: the index is the record.
- **A merge under way** keeps using the shared index for everything, since git keeps the merge
  there. Unchanged from before.

## Commit

Under a lock (`~/.local/share/nit/commit.lock`, also held by `nit update` while it pulls):

1. The paths: this session's staged paths, or the named subset (`nit commit -- <paths>`).
2. **Path-level compare-and-swap**: for each path, the blob at the session's base must equal the
   blob at HEAD now. If another commit changed that path since this session staged it, refuse and
   name the path and the commit: stage it again on top of the new version. If HEAD now holds
   exactly what this session staged (the nightly committed the same always_commit file, say), it
   is no conflict: the path is dropped from the commit and the record, and a commit left with
   nothing to do is a quiet success.
3. Build a temporary index from HEAD and copy in exactly those paths' entries from the session's
   index (removals included).
4. Ack gate for in-scope templates (since `066fea0` it stops only when the deploy could lose something: a hand edit to the deployed file, or a file nit never wrote; README § The Ack System).
5. `git commit` with `GIT_INDEX_FILE` set to the temporary index: hooks run as usual (the
   pre-commit secret gate included), and no pathspec, so the work tree can never override what
   was staged. Afterwards HEAD's parent must be the HEAD read in step 2; if not, say so loudly.
6. The shared index entries of the committed paths are set to the new HEAD (`git reset --
   <paths>`), so `git status` does not show them as staged backwards.
7. The session's index is rebuilt on the new HEAD with whatever it still holds uncommitted; base
   and `last_commit` are updated. Empty → removed.
8. Only now: deploy this session's in-scope templates, decrypt secrets, run triggers. A refused
   commit deploys nothing.

**Amend**: allowed only when HEAD is this session's `last_commit` (and not on the remote, as
before). Otherwise refused, naming whose commit HEAD is.

## Session identity and lifetime

- The anchor is the existing process-tree walk (`get_session_anchor`); `NIT_SESSION_ANCHOR`
  overrides it (tests, and scripts that want an explicit identity).
- **PID reuse**: a session file whose recorded start time differs from the live anchor's is
  another, ended session's; it is moved to `sessions/ended/` and the live session starts clean.
  Never inherited.
- **Lifetime**: created by the first staging command (or `nit session-index`), the index removed
  after a commit that leaves nothing staged. Files of ended sessions move to `sessions/ended/` at the next nit command and are
  deleted after 30 days. What they held is still in the work tree as an ordinary edit, which the
  drift queue reports.

## Moving over

A session that staged with the old nit has entries in the shared index and in the old side
list. The first commit or status after the upgrade adopts those entries into the session's index
once (only paths the side list names), so work in flight survives the upgrade.

## What is unchanged

The pull gate, the drift queue and `nit update` read the shared index and the work tree. Staged
work in a session's index is, to them, an ordinary edit in the work tree, which they already
handle: pulled around, or the pull stops for it.

## Rejected

- **An advisory lock on the shared index** (rejected in May, still rejected): it serialises
  commands but not intent; two sessions' entries still mix in one index.
- **Keeping the side list and patching each command**: the history above.
- **`commit-tree` + `update-ref` with a true compare-and-swap**: exact, but skips git's hooks,
  and the pre-commit hook is the secret gate. The lock plus the parent check gives the same
  result for nit commits; a raw `git commit` landing in the same few milliseconds is reported.
