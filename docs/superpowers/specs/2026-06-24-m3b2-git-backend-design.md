# M3b.2 — Git-fetch resolver backend (design)

**Goal:** let a namespace resolve to a git repository at a ref, instead of only a local directory — completing the supply-chain "location" leg (§5.1) while keeping the verify-before-eval integrity guarantee (D27) byte-for-byte intact.

## 1. Scope

**In:**
- A `git` backend in `.mangrove/resolvers.toml`: `[namespace.<ns>] git = "<url-or-path>"`.
- The reference's `<tag>` (`use "infra/x@v1"`) is the git ref (tag / branch / commit).
- Fetch = clone + checkout into a content-stable cache under the config dir; read `<rest>.mang` from the checkout; the bytes then flow through the **unchanged** verify-before-eval path (hash-checked against `mangrove.lock`).
- Reference-component hardening: validate `<ns>`, `<rest>`, `<tag>` against a safe charset and reject `..` and leading `-`. This also closes the local-backend path-traversal that M3b.1 deferred (§ below).

**Out (explicitly):**
- **Per-type version pins (§5.6).** They require cross-file *type* imports (`schema k.Deployment`, `k.Probe @v4.8`) — importing type definitions, not composed values. That is a separate feature; §5.6 is scheduled after it lands. Noted here so the deferral is intentional, not forgotten.
- Shallow-clone / shared-object optimization, auth (private repos), submodules → later if needed.

## 2. Decisions

- **D30 — git backend.** `[namespace.<ns>] git = "<url>"`. Exactly one of `remote` (local dir, M3b.1) or `git` per namespace; both or neither → config error. `<tag>` is the git ref; `<rest>.mang` is the file within the repo (same rest→file mapping as the local backend). After checkout the source bytes are hash-verified against the lock exactly as for a local file — **a malicious or substituted git remote cannot inject content** (mismatch → hard error), so the git backend is "location" only, never trust.
- **D31 — cache.** A checkout lives at `<config_dir>/.mangrove/cache/<ns>/<tag>/`, created once per (namespace, ref) and reused. The cache dir is not committed (it is regenerable from `url@ref`); `.mangrove/cache/` should be git-ignored by users. Builds (`hash`/`check`) read the cache if present and only clone on a miss; they never update a cached ref in place (a moving branch ref is the user's choice of `<tag>` and is still pinned by hash, so a moved branch whose content changed fails integrity until `mangrove update`).
- **D32 — no shell, validated components.** Git is invoked via `std::process::Command` with explicit args (never a shell), and `<url>` is passed after `--` so it cannot be read as an option. `<ns>`, `<rest>`, `<tag>` must match `[A-Za-z0-9._/-]+`, contain no `..` path segment, and not begin with `-`; otherwise a resolution error. This blocks arg-injection via a hostile `<tag>` and path-escape via `<rest>` — for **both** backends (so M3b.1's deferred local `..` traversal is closed here too).

## 3. Architecture

- **`mangrove-resolve`:** `Resolvers.map` becomes `BTreeMap<String, Backend>` where
  ```
  enum Backend { Local(PathBuf), Git { url: String } }
  ```
  `find_and_load` reads `remote=` → `Local`, `git=` → `Git`, errors if both/neither.
  `resolve_path(reference)` validates components (D32), then:
  - `Local(dir)` → `config_dir.join(dir).join(rest_or_ns + ".mang")` (unchanged behaviour).
  - `Git { url }` → ensure `<config_dir>/.mangrove/cache/<ns>/<tag>/` exists (clone `--quiet -- <url> <dir>`, then `-c advice.detachedHead=false checkout --quiet <tag> --`); return `<cache>/<rest>.mang`.
  A `git_fetch` helper isolates the two `Command` calls and maps non-zero exit / missing-`git` to a clear error.
- **`mangrove-compose` / `mangrove-cli`:** unchanged. They already call `resolve_path` then verify — the backend is opaque to them, so the git path inherits read-once verify (B2), the closed-remote-subtree rule (B1), and the global root lock (D29) for free.

## 4. Testing (hermetic, offline)

A local git repo stands in for the remote — `git clone` from a filesystem path needs no network:
- **happy path:** init a temp repo, commit `x.mang`, `git tag v1`; resolvers `git = "<repo>"`; `mangrove update` pins it; `check` composes the verified import.
- **integrity:** pin a wrong hash → `integrity check failed` (proves git bytes still go through verify).
- **bad ref:** `<tag>` that doesn't exist → clean resolution error (no panic).
- **D32 guards:** `<tag>` = `--foo`, `<rest>` = `../escape` → rejected before any git call.
- **config errors:** namespace with both `remote` and `git`, or neither → error.
- **cache reuse:** second resolve of the same (ns,ref) does not re-clone (assert by removing the source repo after the first resolve and seeing the second still succeed from cache).

## 5. Out-of-scope reminder

§5.6 per-type pins and cross-file type imports are a separate, later milestone. M3b.2 finishes the *location* backend story; the *integrity* model (D27) and *namespace* model (D29) are unchanged.
