# M3b.1 — Resolver, Lockfile & Hash-Verification: Design

> **Status:** Proposed — 2026-06-24
> **Milestone:** M3b.1 (first resolver slice; M3b.2 = the real git-fetch backend + per-type pins §5.6).
> **Normative source:** `mangrove-spec.md` §5.1 (identity/location/auth split), §5.2 (lockfile & integrity), §11.2 (hash-verified imports).
> **Goal:** Resolve **namespaced** imports through the identity/location/auth split — `use "infra/k8s/core@v5.0" as k`, a committed `mangrove.lock` (tag → content hash), a non-committed `.mangrove/resolvers.toml` (namespace → location), and the load-bearing security property: **fetch bytes by reference, verify their hash against the lockfile *before* evaluation, fail closed on mismatch.** The backend in M3b.1 is a local directory (hermetically testable); the real git fetch is M3b.2.

Builds on M3a (local `use` + compose). A namespaced `use` resolves to bytes via the resolver, is hash-verified against the lockfile, then composes exactly like a local `use`.

---

## 1. Scope

**In:**
- **Namespaced `use`** (§5.1): `use "<namespace>@<tag>" as alias` (e.g. `"infra/k8s/core@v5.0"`). Local relative `use "./x.mang"` (M3a) still works; the leading `./`/`../` distinguishes local from namespaced.
- **`.mangrove/resolvers.toml`** (§5.1): non-committed, maps a namespace's first segment to a location. M3b.1 backend: a **local directory** (`remote = "/abs/or/rel/dir"`).
- **`mangrove.lock`** (§5.2): committed, maps `"<namespace>@<tag>" → "b3:<hash>"` (the BLAKE3 of the imported source bytes).
- **Hash-verify-before-eval** (§5.2, §11.2): the resolver reads the bytes, hashes them, and compares to the lockfile entry **before parsing/composing**. A mismatch, or a missing lockfile entry, is a hard failure.
- **`mangrove update`**: a new command that (re)resolves every namespaced `use` reachable from a document and writes/updates `mangrove.lock` with the resolved hashes. (Without it the lock can't be bootstrapped; with it, re-resolution is the deliberate act §5.2 describes.)

**Out (deferred to M3b.2 / later):**
- The **git-fetch backend** (shelling out to `git`, tags→commits→trees, credential machinery) → M3b.2. M3b.1's backend is a local directory standing in for a remote.
- **Per-type version pinning** (§5.6, `k.Probe@v4.8`) → M3b.2.
- A shared checksum-log server (optional resolver feature, never required) → not planned.
- Path-traversal sandboxing for local backends — noted in the M3a threat model; the resolver boundary is where containment would live, tracked but not enforced in M3b.1.

---

## 2. The three-place split (§5.1)

```
# in the document — identity + intent only (no location, no credentials)
use "infra/k8s/core@v5.0" as k
use "./base.mang"          as base    # local (M3a), no namespace

# mangrove.lock — committed, CI-verified: the pin (tag → content hash)
"infra/k8s/core@v5.0" = "b3:7e1f2a9c…"

# .mangrove/resolvers.toml — NOT committed: location (+ auth, later)
[namespace.infra]
remote = "../vendor/infra"     # M3b.1: a local directory; M3b.2: a git URL
```

- **Decision D24 — namespaced use syntax** is `use "<ns>@<tag>" as alias` (quoted; the `@tag` lives inside the string, split on the last `@`). A path beginning `./` or `../` is **local** (M3a path, no `@tag`); anything else is **namespaced** and requires a resolver entry + lockfile pin.
- **Decision D25 — resolver config** is `.mangrove/resolvers.toml`, found by searching from the **root document's directory upward** to the filesystem root (first hit wins). Parsed with the `toml` crate. `[namespace.<first-segment>] remote = "<local-dir>"`. The import `<first>/<rest>@<tag>` resolves to `<remote>/<rest>.mang` (M3b.1 ignores `<tag>` for file location — it is the git ref in M3b.2; here it is only a lockfile key). A missing namespace entry is an error naming the namespace and the expected config.

---

## 3. The lockfile & integrity (§5.2, §11.2)

- **Decision D26 — `mangrove.lock`** lives at the project root (the directory containing `.mangrove/`, or the root document's directory), committed, TOML: `"<ns>@<tag>" = "b3:<hex>"`. The hash is **BLAKE3 of the imported document's raw source bytes** (the "bytes verified before eval" of §5.2 — distinct from a value's canonical content address; it pins the exact source).
- **Decision D27 — verify before eval (fail closed):** resolving a namespaced `use` (a) maps it to bytes via the resolver, (b) computes `b3:` of those bytes, (c) looks up the lockfile entry for `"<ns>@<tag>"`:
  - entry present and **matches** → parse + compose the bytes.
  - entry present and **mismatches** → hard error (`integrity check failed: <ns>@<tag>`), *before* any parsing/evaluation. A substituted/compromised source cannot inject content.
  - entry **absent** → error (`<ns>@<tag> not in mangrove.lock; run \`mangrove update\``).
  Builds (`hash`/`check`) read the lock; they never silently re-resolve. This is the supply-chain guarantee from the committed lockfile alone — no checksum server.
- **`mangrove update <file>`:** resolve every reachable namespaced `use`, compute each source's `b3:`, and write/merge `mangrove.lock`. This is the deliberate re-resolution step; it is the only path that writes the lock.

---

## 4. Architecture / crates

- **New crate `mangrove-resolve`** (or a `resolve` module in `mangrove-compose`): `resolvers.toml`/`mangrove.lock` parsing (via `toml`), namespace→location mapping (local-dir backend), and `resolve_and_verify(reference, root_dir) -> Result<(PathBuf | bytes), Error>` that fetches + hash-verifies. deps: `toml`, `blake3`.
- **`mangrove-compose`:** `compose_rec` distinguishes local vs namespaced `use`; namespaced ones go through the resolver (verify, then recurse on the resolved bytes/path). The `visiting` cycle/depth guards extend to namespaced imports (keyed by resolved path/hash).
- **`mangrove-cli`:** add `mangrove update <file>`; `hash`/`check` already compose, now also resolve namespaced imports through the verifying resolver.
- **`mangrove-conformance`/tests:** hermetic — write a temp project (`root.mang`, `.mangrove/resolvers.toml` pointing at a temp "remote" dir, `mangrove.lock`), assert: verified import composes; tampered source → integrity error; missing lock entry → error; `update` writes a correct lock.

New workspace deps: `toml = "0.8"`.

---

## 5. Decisions to confirm

- **D24** — namespaced `use "<ns>@<tag>" as alias` (quoted, split on last `@`); `./`/`../` = local (M3a).
- **D25** — `.mangrove/resolvers.toml` (found by upward search from the root doc), `toml`-parsed, `[namespace.<seg>] remote = <local-dir>` in M3b.1; `<rest>.mang` under the remote; `<tag>` is a lockfile key only here.
- **D26** — `mangrove.lock` at project root, committed, `"<ns>@<tag>" = "b3:<hash-of-source-bytes>"`.
- **D27** — verify-before-eval, fail closed: mismatch or missing-entry → hard error before parsing; builds read the lock, never auto-resolve; `mangrove update` is the only writer.
- Git-fetch backend + per-type pins → **M3b.2**.

---

## 6. Post-review hardening (D28) and known limits

Adversarial security review of the implemented resolver surfaced three integrity-bypass paths; the fixes are part of M3b.1:

- **Read-once verify (no TOCTOU).** `compose_rec` reads a file's bytes once, verifies *that* buffer against the lock, then parses the *same* buffer — it never re-opens the path. A previous design verified one read and composed a second read, leaving a swap window.
- **Closed remote subtree (B1).** Once composition crosses a namespaced (remote) import, every file in that subtree is pinned; an unpinnable local `./`/`../` import *inside* a remote package is a hard error (`a remote package may not use the local import …`). Otherwise a pinned entry file could pull in unpinned sibling content. `mangrove update` enforces the same rule so the lock it writes matches what `compose` accepts. Multi-file remote packages must therefore compose their pieces via namespaced references (each pinned); relative-path packaging from a remote is deferred to M3b.2's content-addressed closure pinning.
- **TOML-crate serialization (S4).** `mangrove.lock` is written via the `toml` crate, not `{:?}`, so a reference containing control characters round-trips instead of emitting invalid TOML that the reader then rejects.

**D29 — Global namespaces, one root lock (resolves S3).** The `.mangrove/resolvers.toml` namespace map and the `mangrove.lock` are discovered once at the root document's directory and govern the *entire* import graph; a transitively-imported package's own `resolvers.toml` is never consulted. This is the `Cargo.lock` / `package-lock.json` model: one resolution context, every reference (`"<ns>@<tag>"`) globally unique, every file in the transitive closure pinned in the single root lock. It is **collision-free by construction** — a reference resolves to exactly one location (the root's namespace map) and that one file's bytes are pinned — and it was never an integrity gap, since every file is hash-verified regardless (a genuine namespace clash surfaces as an ordinary resolution/validation error, never silent unverified content). The accepted cost: the root project must declare every namespace used anywhere in its closure. Per-package namespace isolation (each package resolving deps in its own context) is explicitly **not** supported and is not planned; if it is ever needed it is additive, not a correction.
