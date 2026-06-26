# Migrating existing config to Mangrove

A practical, incremental path for moving a project's Kubernetes YAML, Helm charts,
and other YAML/TOML configs (e.g. pyinfra) onto Mangrove — without a big-bang
rewrite, and with a content-hash safety net that proves each step changed nothing.

The whole approach rests on one property: **every Mangrove document reduces to a
canonical value with a BLAKE3 hash, and `import`/`export`/`fmt` are
meaning-preserving.** So at each step you can assert *equivalence* rather than
eyeballing a diff.

> Verified on this repo's `examples/real-world/deployment.yaml`: `import` → `.mang`,
> round-trip back through `export`, and `fmt` in place all produce the **same**
> `b3:…` hash.

---

## Phase 0 — adopt as a checker, rewrite nothing

Drop the `mangrove` binary on `$PATH` and use it to sanity-check existing YAML.
The importer rejects YAML's footguns (no `null`, no `no→false`, exact numbers —
no f64), so this alone surfaces latent bugs:

```sh
mangrove import deployment.yaml > deployment.mang   # schemaless, exact, null-free
mangrove export deployment.mang --to yaml           # round-trips; hashes match
```

If `import` errors, that's a real problem in the source YAML (a stray null, a
duplicate key, an ambiguous scalar) — fix it at the source.

## Phase 1 — type your Kubernetes manifests

Generate Mangrove types from your *actual* cluster's OpenAPI, then bind a schema
so a typo becomes a build error instead of a silent prod incident:

```sh
kubectl get --raw /openapi/v2 > k8s-swagger.json
mangrove gen-openapi k8s-swagger.json --root io.k8s.api.apps.v1.Deployment > k8s-types.mang
```

Then in your imported doc, `use` the generated types and `schema k.Deployment`,
and run `mangrove check`. Out-of-range `replicas`, a misspelled field, a wrong
port — all now fail `check`.

> **Caveat (from `k8s/README.md`):** CRDs with recursive `JSONSchemaProps` can't be
> fully typed under Mangrove's no-recursion axiom — they degrade to an opaque
> `Json` type with a warning. That's fine; you still type everything else.

## Phase 2 — replace Helm templating

This is the big win and Mangrove's whole thesis: **Helm templates *text*, Mangrove
templates *values*** — so mis-indentation can never corrupt structure, because you
don't produce text until the final `export`.

Translate, roughly:

| Helm | Mangrove |
|------|----------|
| `values.yaml` | `params { … }` (a default = optional; no default = required) |
| `{{ .Values.x }}` | a bare reference `x`, or `"…${x}…"` interpolation |
| `{{ if eq .Values.env "prod" }}` | `match env { prod: …, dev: …, _ : … }` (total) |
| `_helpers.tpl` partial | a schema-defined `fn` constructor, or a `use`d module |
| chart dependency | a `use "ns/chart@v1"` import, pinned in `mangrove.lock` |

See [`examples/k8s-templated.mang`](../examples/k8s-templated.mang) for exactly this
pattern: per-env replicas via `match`, image tag via `${version}`, memory via a
`unit Mem`.

## Phase 3 — pyinfra and other YAML/TOML configs

Same `import` → (optionally type) → `export` loop. If the consuming tool wants
TOML, `export --to toml` works too. Numbers stay exact, so no precision drift.

---

## A worked round-trip (real data)

```sh
$ mangrove import examples/real-world/deployment.yaml > dep.mang
$ mangrove hash dep.mang
b3:b13a095b15e237364de34c62f2ea5c0c35110eecbcd348369b07b28276259d17

$ mangrove export dep.mang --to yaml | mangrove import /dev/stdin | mangrove hash /dev/stdin
b3:b13a095b15e237364de34c62f2ea5c0c35110eecbcd348369b07b28276259d17   # identical

$ mangrove fmt dep.mang && mangrove hash dep.mang
b3:b13a095b15e237364de34c62f2ea5c0c35110eecbcd348369b07b28276259d17   # fmt preserves meaning
```

The migration superpower: hash the rewritten `.mang` and compare to the imported
original. **Equal hashes ⇒ provably the same config**, regardless of key order or
formatting. Migrate-and-verify, not migrate-and-hope.

### Known rough edge

`mangrove import` emits the whole document inline (one long line), and `fmt`
preserves author line breaks rather than reflowing — so it won't auto-expand that
line. After importing, break records onto their own lines by hand (or keep them
inline); `fmt` then keeps your layout tidy. The hash is unaffected either way.

---

## Wiring into your `justfile`

Copy the recipes from [`docs/migrate.just`](migrate.just) into your project's
`justfile` (or import it). They cover: bulk `import` of a YAML tree, a `check`
gate, a `fmt --check` CI gate, `render` (export to YAML), and a `verify` recipe
that asserts a `.mang` still hashes to a recorded value.
