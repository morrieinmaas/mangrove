# M7 — Per-package resolver/lock anchoring (re-opening D29)

**Goal:** let each package resolve and verify *its own* dependencies against *its own* `.mangrove/resolvers.toml` + `mangrove.lock`, instead of the root's governing the whole graph. This supersedes D29 (global namespace, one root lock) with the nested-lock model (each package pins its direct deps, like a vendored Cargo/npm workspace where a dependency carries its own lock).

## What changes

- **D50 — per-package anchoring (supersedes D29).** Namespaced resolution and lock-verification are anchored at the *importing package's* root, not the document's root:
  - The **root** document resolves its namespaced `use`s via the resolvers found upward from the root, and verifies them against the root's `mangrove.lock` (unchanged for a one-level project — fully backward-compatible).
  - When composition enters a **fetched package** (a namespaced import), that package's own deps resolve against *that package's* `.mangrove/resolvers.toml` and verify against *that package's* `mangrove.lock`, both re-discovered by searching upward from the fetched file's directory (bounded at the package root / fetch cache). A package therefore brings its own namespace map and its own committed lock.
- **Verification chain.** An import's bytes are verified against the *importer's* lock (the importer pins its direct dep). That dep's own deps are then verified against the *dep's* lock. The fail-closed property (D27) holds at every level: a missing/mismatched entry in the relevant package's lock is a hard error.
- **`mangrove update`** manages only the **root's** lock (the root's direct + transitive-through-root deps it can see). A vendored package ships its *own* committed `mangrove.lock`; you do not regenerate a dependency's lock from the consumer. (If a dep's lock is absent, its namespaced uses fail closed — the dep must be published with its lock.)

## Why this is safe

- **Backward compatible.** For any existing project (root resolvers/lock, deps without their own `.mangrove/`), behavior is identical: the root still governs its direct deps; a dep with no own resolvers simply can't make namespaced imports (it errors "no resolver", exactly as today). All current tests pass unchanged.
- **B1 (closed remote subtree) stays.** A `./`/`../` import inside a fetched package is still refused — it is unpinnable by either the consumer or the package's namespace lock. Per-package anchoring only changes how *namespaced* deps resolve, not the local-import rule.
- **D27 still fail-closed at every level.** Each package's lock gates its own direct deps before their bytes are parsed/evaluated.
- **No global-namespace collision.** Two sibling packages may now legitimately use the same namespace segment (`dep`) for *different* repos, because each resolves `dep` via its own resolvers — the exact confusion D29 ruled out is now resolved correctly per package.

## Architecture

`compose_rec` currently threads one `(resolvers, lock)` pair for the whole graph. M7 makes it re-anchor on each remote boundary:

- `compose_rec` keeps using the *current* package's `resolvers`/`lock` for this file's direct `use`s.
- For a **local** (`./`) import: same package → same `resolvers`/`lock` (unchanged).
- For a **namespaced** import: resolve via the *current* `resolvers`; verify the fetched bytes against the *current* `lock`; then, for the recursive `compose_rec` over the fetched file, re-discover `resolvers`/`lock` anchored at the fetched file's directory (`Resolvers::find_and_load` / `Lockfile::find_and_load` from that dir) and pass *those* down. A fetched package with no own `.mangrove/` gets empty resolvers + an empty lock anchored at its dir (its own namespaced uses then fail closed).
- `lock_references`/`mangrove update` walk only within the root's anchoring (they produce the *root's* lock); they do not descend into a dependency's own-lock territory to rewrite it.

This is a localized change to the compose driver; the resolver/lockfile types (`find_and_load`, `verify`) already support discovery-from-a-directory, so no new resolver machinery is needed.

## Testing (hermetic)

- A root that imports package A (namespaced); A has its *own* `.mangrove/resolvers.toml` + lock and imports package B under a namespace the root does NOT define — composes correctly via A's resolvers (proves per-package resolution).
- Two sibling packages each using namespace `dep` for *different* local backends — each resolves its own `dep` (proves the D29 collision is gone).
- A dep whose own lock is missing/mismatched for ITS dep → fail closed at the dep level.
- Backward-compat: every existing namespaced/git/lock test still passes unchanged.
- The verification chain: tampering a transitive dep's bytes fails against the relevant package's lock.

## Out of scope

- Cross-file *type* imports from a dep's own deps (nested type imports) — M6 is one level; this stays.
- A workspace-wide lock override flag — not needed; per-package is the model.

---

## Post-review note (M7)

The M7 adversarial review came back clean: no unverified bytes reach eval, no
wrong-lock substitution, no consumer-config leak (verified for both backends,
including the git checkout physically nested under the consumer's
`.mangrove/cache/`), fail-closed at every level, read-once/no-TOCTOU preserved,
and all existing tests unchanged.

One containment nuance (by design, not a finding): a dependency's *own*
`resolvers.toml` may set `remote`/`git` to a path outside its own subtree (e.g.
`../..`), and those values are not path-confined. But every file so read is still
hash-gated by that package's own lock — a stale/wrong pin fails closed
(`integrity check failed`). So a dependency can neither introduce unpinned content
nor read bytes whose hash it does not already pin. This is the same trust model as
pre-M7 (the root's resolvers could always point `remote` anywhere); D50 only
localizes it per package. Hard subtree-confinement of `remote`/`git` values would
be a new constraint beyond D27's promise, not a weakening of it.
