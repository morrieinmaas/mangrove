# Mangrove

A typed, composable, content-addressed configuration language — implemented in Rust.

Mangrove is built in layers: plain **data** (L0), a **type system** (L1), **composition** (L2),
and **templating** (L3), on top of a supply-chain layer for verified imports. Every document
reduces to a single canonical value with a stable BLAKE3 content hash, so two documents that
*mean* the same thing hash the same.

## Design axioms

- **No surface type inference** — the schema is the sole type authority, enabling one canonical form.
- **No null** — absence is expressed by composition (`unset`), never a null value.
- **Arbitrary precision** — integers and decimals are exact (`BigInt`/`BigDecimal`); no IEEE float.
- **Content-addressed** — the canonical form is deterministic CBOR (RFC 8949 §4.2/§7.1) → `b3:<hex>`.
- **Verify before eval** — namespaced imports are hash-verified against a committed lock, fail-closed.

## The layers

| Layer | What it adds |
|-------|--------------|
| **L0 — Data** | maps, lists, strings (incl. text blocks & raw), ints, decimals, bools, bytes |
| **L1 — Typed** | `type`/`schema`, refinements (`int & >= 1 & <= N`), unions, literals, units (`512Mi`), brands, `require`, annotations (`@key`, `@message`, `@deprecated`), defaults |
| **L2 — Composed** | `use` + spread (`...alias`), deep merge, `unset`, `@key` list-ops, subtype redefinition (`schema Base & {…}`) |
| **L3 — Templated** | `params`, references, string interpolation (`${v}`), `match`, schema `fn` constructors, module calls (`emit: webapp(...)`) |
| **Supply chain** | resolver split (identity/location/auth), local + git backends, `mangrove.lock` hash-verify, per-package anchoring, cross-file type imports, per-type version pins |
| **Interop** | YAML/TOML ⇄ Mangrove converters (`import`/`export`) |

## CLI

```
mangrove hash   <file.mang>            # the BLAKE3 content address of the canonical value
mangrove check  <file.mang>            # validate against the bound schema
mangrove update <file.mang>            # resolve + pin namespaced imports into mangrove.lock
mangrove import <file.yaml|.toml>      # convert YAML/TOML to a schemaless Mangrove document
mangrove export <file.mang> --to yaml  # evaluate and emit YAML/TOML
```

## Example

```
type Server = { host: str, port: int & >= 1 & <= 65535 }
schema Server

host: "api.example.eu"
port: 8443
```

See [`examples/`](examples/) for a Kubernetes Deployment (with units, refinements, `@key` lists),
a templated per-environment Deployment (`params` + `match` + interpolation), and a `pyproject`.

## Building

Rust 2024 edition (≥ 1.85). The build is gated by `just ci` (fmt-check → clippy `-D warnings`
→ build → test). `unsafe` code is forbidden workspace-wide.

```
just ci      # the full gate CI runs
just test    # tests only
```

## License

Apache-2.0.
