# Mangrove — Language Specification

> **Status:** Draft 0.1 (name **Mangrove**, file extension `.mang`)
> **Layer scope:** L0–L3, extensible to Ln
> **Canonical encoding:** UTF-8 text (truth) ↔ CBOR (wire), addressed by BLAKE3
> **License intent:** spec and conformance suite under an OSI-approved license; no source-available restrictions

This document is the normative reference. The companion `mangrove-rfc.md` carries the motivation, rationale, and comparison to prior art; where this spec says *what*, the RFC says *why*.

---

## 1. Overview

Mangrove is a typed configuration language with a single canonical, content-addressable form. It is designed around one axiom from which nearly everything else follows:

> **No inference.** A bare token carries no type. Types come from a schema, never from the surface syntax of a value. The schema is the sole authority on what a value means.

This kills the entire class of bugs that surface-type inference creates (YAML's `NO` → `false`, `1.20` → `1.2`, sexagesimal timestamps), and it is what makes a stable canonical form — and therefore hashing, signing, caching, and semantic diff — possible.

Mangrove is defined as four concentric layers. Each layer adds power; a document pays — in concepts it must understand — only for the layers it actually uses. The layers are one language with one canonical form, not separate dialects.

| Layer | Name | Adds | Replaces |
|------|------|------|----------|
| **L0** | Data | braces, scalars, lists, maps, comments, units-as-numbers | JSON, TOML |
| **L1** | Typed | `schema`, refinement types, unit types, unions, `match`, optional fields, **custom types** | hand-written typed config |
| **L2** | Composed | `use`, spread/override, `@key` list ops, `unset`, lockfile, subtype redefinition | Kustomize |
| **L3** | Templated | `params`, total schema-defined functions, interpolation | Helm |

A conforming implementation **may** target a subset of layers (e.g. an L0/L1 reader is small and complete), but the reference implementation targets all of them, and the canonical form is identical across layers.

---

## 2. Design axioms (normative)

1. **No inference.** Surface tokens are untyped; the schema assigns type.
2. **Braces are truth; whitespace is cosmetic.** Structure is delimited by `{ }` and `[ ]`. Indentation is never significant and is owned entirely by the formatter. Tabs vs. spaces cannot be a defect because neither is read by the parser.
3. **One canonical form.** Every document has exactly one normalized byte representation, hashed with BLAKE3. Two documents with identical semantic content have identical hashes.
4. **No null.** A key is *present-with-a-value* or *absent*. There are exactly two states, never three.
5. **Arbitrary precision.** Integers are arbitrary precision; non-integers are decimal. IEEE-754 floats never enter the model.
6. **Totality.** All computation (templates, functions, predicates) is total and decidable: no recursion, no user-defined Turing-complete functions, guaranteed termination.
7. **Explicit over implicit.** Composition, merging, and overriding are never inferred from structure; they are named operations or follow one stated rule.

---

## 3. L0 — Data

### 3.1 Lexical

- Encoding is UTF-8.
- Whitespace (spaces, tabs, newlines) separates tokens and is otherwise insignificant. The formatter normalizes it.
- Entries in a record or map are separated by newline **or** comma; trailing commas are legal.
- Comments: see §3.6.

### 3.2 Primitive types (the axiom set)

These are built in. They are the foundation the type-of-types stands on; they are enumerated here rather than derived. **User-defined types (L1, §4) extend this set freely** — the axioms are a floor, not a ceiling.

| Primitive | Meaning |
|-----------|---------|
| `int` | arbitrary-precision integer |
| `decimal` | arbitrary-precision decimal (no binary float) |
| `str` | UTF-8 string |
| `bool` | `true` / `false` |
| `bytes` | byte string (base64 in text form) |

Three composite constructors are also primitive:

| Constructor | Form | Meaning |
|-------------|------|---------|
| record | `{ field: value, … }` | fixed, individually-typed fields |
| list | `[ value, … ]` | ordered, homogeneous-by-schema sequence |
| map | `{ key: value, … }` with dynamic keys | uniform value type, dynamic string keys |

> Records and maps share brace syntax but are **distinct kinds**: a record has named fields known to the schema; a map has dynamic keys (`[str]: V`). The distinction is resolved by the schema, and it is the line between "I know every key" (strong) and "any key, uniform value" (open). At L0 with no schema, `{ … }` is read as a map.

### 3.3 Scalars

```
"a string"            # str
42                    # int
3.14                  # decimal (never a float)
true  false           # bool
b64"3q2+7w=="         # bytes
```

No bare `null`, no `~`, no unquoted barewords-as-booleans. A bare word that is not `true`/`false` is a syntax error at L0 (it would be a string only inside quotes).

### 3.4 Strings

```
"plain with ${interpolation}"     # interpolation active (see §6.3)
r"raw, no escapes, no ${x}"       # raw: escapes and interpolation disabled
"""
  text block; the column of the closing delimiter
  sets the left margin stripped from every line
  """                              # → two lines, dedented to the closing-quote column
r"""…"""                           # raw text block
```

Text-block margin is defined by the **column of the closing `"""`**, not by an indentation indicator. This is deterministic and paste-safe, and it does not reintroduce structural whitespace because the margin is fixed by an explicit token position.

### 3.5 An L0 document

```
{
  name: "forgitry"
  replicas: 6
  ports: [ 8443, 9090 ]
  labels: { app: "api", tier: "edge" }
  notes: """
    multi-line
    free text
    """
}
```

This is valid standalone — a JSON superset with comments, trailing commas, and honest numbers. With no `schema` binding it is pure data: every scalar is exactly the literal kind written, nothing is coerced.

### 3.6 Comments — three kinds, three fates

| Syntax | Name | Semantic? | In the hash? |
|--------|------|-----------|--------------|
| `# …` | ordinary | no | **no** — lives on the text projection only |
| `## …` | doc | yes — sugar for `@doc`, attaches to a schema field | **yes** (part of the schema contract) |
| `#! …` | directive | yes — instructs tooling (version, schema binding, pragmas) | **yes** |

```
#!mangrove 0.1
#!schema infra/k8s/core@v5.0#Deployment

## listening port; must be free on the host
port: 8443        # ordinary inline comment — not hashed
```

Rule of thumb: `#` is for humans and does not count; `##` documents a schema field and does; `#!` instructs tooling and does. There are no block comments.

---

## 4. L1 — Typed

### 4.1 Binding a schema

```
#!schema infra/k8s/core@v5.0#Deployment
# or, as a keyword form:
schema k.Deployment
```

Once a document is schema-bound, every value is checked against the schema. Bare scalars are now legal because the schema types them; without a schema (L0) you stay in pure-data mode.

### 4.2 One type grammar; a name is an abbreviation

There is a single type-expression grammar. A named type (`type X = …`) is nothing more than an inline type with a name bound to it. This is the only thing needed to support both "TypeScript `interface`/`type` alias" style and "inline annotation" style — they are the same grammar with the name slot filled or empty.

```
type Env    = "dev" | "staging" | "prod"             # union of literals
type Port   = int & >= 1 & <= 65535                  # refinement
type Listen = { host: str, port: Port, tls?: bool }  # record; tls optional
type Tags   = { [str]: str }                          # map: dynamic keys → str
type Hosts  = [ str ]                                  # list
```

The three line shapes, distinguished only by which slots are present:

```
env : Env                 # type only            → declaration / interface
env : Env = "prod"        # type + value         → field with default
env       = "prod"        # value only           → plain config (type from schema)
```

### 4.3 Refinements and unions

A type is a value-with-constraints, written in the same language as data.

```
type Port     = int & >= 1 & <= 65535
type LogLevel = "debug" | "info" | "warn" | "error"
type Host     = str & =~ "^[a-z0-9.-]+$"
```

Refinement predicates allowed in *type position* (and therefore subject to the decidable subtype check, §5.5) are: interval bounds on `int`/`decimal`, finite enum membership, and regex match on `str`. Richer cross-field assertions use `require` (§4.7), which is validated against values but **not** implication-checked between types.

### 4.4 Custom types are first-class and open

The primitive set (§3.2) is a floor. Users define new types freely, and those types are indistinguishable in standing from built-ins:

```
type Email  = str & =~ "^[^@]+@[^@]+$"  @doc("RFC 5322-ish")
type UserId = brand int & >= 1
type Money  = { amount: Sats, currency: "BTC" | "EUR" }
```

A type may be **structural** (default — a pure abbreviation; any value of the right shape qualifies) or **branded** (§4.6, a distinct nominal identity).

### 4.5 Unit types

Units are brands for numerics with a declared, discoverable literal grammar. The standard set ships in `std/units`; custom units are one line.

```
use std/units { Bytes, CPU, Duration }      # the common path

unit Bytes : int { B = 1, Ki = 1024B, Mi = 1024Ki, Gi = 1024Mi }
unit CPU   : int { m = 1, core = 1000m }
unit Sats  : int { sat = 1, btc = 100_000_000sat }
```

Rules:

- A literal's suffix **must** be a declared member of the unit type, so `256MB` against `Bytes` is an error: *"unknown unit `MB`; valid: B, Ki, Mi, Gi."* There is nothing to guess; the editor autocompletes the legal set.
- A value stores its canonical base integer; `512Mi` and `536870912` are the same value, comparable and arithmetic-able.
- **Canonicalization rule:** render in the largest declared unit that keeps the value an exact integer. `536870912` → `512Mi`; `1000m` → `1core`; `250` (CPU base) → `250m`.
- Fractional literals are legal **iff** they resolve to an exact integer in the base unit: `0.5btc` = `50_000_000sat` ✓; `0.5sat` is an error.
- Distinct unit types do not mix: a `CPU` value cannot land in a `Bytes` field. Units *are* brands; see §4.6.

### 4.6 Brands (nominal newtypes)

Structural typing is the default. A `brand` gives a type a distinct identity even when its shape is identical to another's.

```
type Satoshis      = brand int & >= 0
type Millisatoshis = brand int & >= 0      # NOT interchangeable with Satoshis
```

Construction ceremony is **inferred away at known slots**: a field typed `Satoshis` receiving a bare literal `21000` brands it automatically (no ambiguity of intent). Ceremony appears only at the genuinely dangerous moment — moving an *already-branded* value into a *different* brand's slot — which is a compile error. The common case (plain literals into typed config) stays clean; the money/units bug is the one thing forced to be explicit.

### 4.7 Cross-field constraints (`require`)

`require` is a total, side-effect-free predicate over fields in scope. Allowed builtins: comparisons, `&&`/`||`/`!`, `len`, regex match, set/enum membership. No user functions, no recursion.

```
type Listen = {
  host:  str
  port:  Port
  tls:   bool | *false           # *false = default
  certs: [ str ]
  require: tls == false || len(certs) >= 1   @message("tls requires at least one cert")
  require: host != "0.0.0.0" || tls == true
}
```

`require` is evaluated against concrete values at validation time. It is **not** used in subtype implication (§5.5) — that would be undecidable.

### 4.8 `match` (exhaustive selection)

`match` selects over a closed union and is checked for exhaustiveness.

```
replicas: match env {
  dev:     1
  staging: 2
  prod:    6
}                              # omitting `prod` is a compile error: "unhandled case: prod"

log_level: match env { dev: "debug", _: "warn" }   # _ is an explicit, visible fallback
```

The wildcard `_` is allowed in values but **lint-flagged in schema definitions**, so silencing the exhaustiveness checker is always a deliberate, visible choice.

### 4.9 Metadata annotations

Types are declared with `=`; non-type metadata attaches with `@`.

```
type Port = int & >= 1 & <= 65535
  @doc("listening port")
  @message("port must be between 1 and 65535")

image: str  @deprecated("use image_ref")
```

`@message` is surfaced in the structured validation error channel (§11).

---

## 5. L2 — Composed

### 5.1 Modules and references — identity, location, and auth are separate

An import is split into three concerns that live in three places. The document carries **identity and intent**; a committed lockfile carries **the pin** (reproducibility); a non-committed resolver config carries **location and auth**. The import string never contains a hostname, a URL, or a credential.

```
# in the document — identity (namespace/path) + intent (@version), nothing else
use infra/k8s/core @v5.0   as k
use acme/internal  @v2.1   as a
use ./base.mang           as base    # local path, no namespace needed
```

```
# mangrove.lock — committed, CI-verified. The pin: tag -> content hash.
"infra/k8s/core@v5.0" = "b3:7e1f2a9c…"
"acme/internal@v2.1"  = "b3:91c4d0…"
```

```
# .mangrove/resolvers.toml — NOT committed; per-machine / per-CI. Location + auth.
[namespace.infra]
remote = "https://github.com/example-org/k8s-core"

[namespace.acme]
remote = "git@git.internal.acme.eu:platform/schemas.git"   # private; auth via SSH agent
```

Rationale (this is a deliberate departure from Go-style imports, where the import string *is* the network location):

- **Private repos need no special-casing.** The document never asserts a public location, so there is no `GOPRIVATE`-style glob to maintain. Public vs. private is purely a resolver distinction, invisible to the document and lockfile.
- **Moving or mirroring a dependency is a one-line resolver change**, not an edit to every importing document. The lockfile hashes (the *identity*) are unchanged, so nothing rebuilds incorrectly.
- **Auth stays in git's existing machinery** (SSH agent, credential helper, deploy keys), referenced by the resolver, never by the document and never by the committed lockfile.
- **Committed artifacts are safe to publish.** Document and `mangrove.lock` contain identities and hashes only — no URLs-with-tokens, no internal hostnames. A repo with private dependencies can be open-sourced without leaking where they live.

A default resolver maps well-known namespaces (e.g. `gh/owner/repo`) straight to public hosts, so public-only projects need no resolver config at all. `mangrove` can emit a `resolvers.toml.example` listing exactly which namespaces a clone must configure, so onboarding a new machine is a known, finite step rather than an env-var hunt.

### 5.2 The lockfile and integrity

A **tag is mutable, a hash is immutable.** The lockfile records the hash a tag resolved to at resolution time. Builds read the lock, not the live tag, so they are reproducible even if a tag is re-pointed. Re-resolution is a deliberate `mangrove update`.

Integrity is independent of location: the resolver fetches *bytes* from wherever the resolver config points, and those bytes are verified against the lockfile hash **before** evaluation. A compromised or substituted mirror cannot inject content, because the hash will not match — so the supply-chain guarantee of a checksum database is obtained from the committed lockfile alone, with no checksum-database *server* required. (A shared team checksum log may exist as an optional resolver feature; it is not required for safety.)

### 5.3 Composition — one rule, one exception

**The rule:** later statements win; records deep-merge. Spread (`...x`) is bulk assignment ("paste these key-values here"), so scalar override and spread are the *same* mechanism.

```
...base                     # paste base's fields
replicas: 6                 # later statement wins
log_level: "warn"
```

**The exception — lists.** Bare list assignment **replaces** (consistent with "bare assignment always replaces"). A list of records annotated `@key(field)` in the schema opts into a named operation block:

```
# schema:  containers: [ Container ]  @key(name)

containers {
  patch "api":  { image: "api:1.21.0", ports += [ 9090 ] }  # deep-merge into element name=="api"
  append:       { name: "envoy", image: "envoy:1.31" }       # add (error if key exists)
  remove:       "cron"                                        # list-level unset by key
}
```

`ports += [ 9090 ]` appends to a list; a plain `ports: […]` replaces it. Every list mutation is therefore either the default (replace) or a *named, greppable* verb — never inferred from structure. This is the single irreducible exception, and it is named as exactly one exception.

### 5.4 `unset`

`unset` is not a separate mechanism; it is the value meaning "absent", legal anywhere a value is. In an overlay it removes an inherited field; the result is **absence, not a present null**.

```
...base
debug_port: unset      # base had it; this document does not
```

`unset` on a schema-required field is an error.

### 5.5 Subtype redefinition

A document may **narrow** a type locally — never loosen, never change it. The constraint is `New <: Old`: every value valid under New was valid under Old. The `&` operator here is **type-only** (its former value-level "must agree" meaning is now just a `require`).

```
use infra/base @v1 as base

schema base.Deployment & {
  replicas: int & >= 1 & <= 10     # narrow base's `int` to a subtype
}
```

The subtype relation is **structural, covariant, depth-recursive**:

- **Scalars:** `int & P <: int & Q` iff `P ⟹ Q`. For intervals and enums this is containment/subset (trivially decidable); for regex it is regular-language containment (decidable, PSPACE).
- **Records:** for every field `f` of Old, `New.f <: Old.f`; New's field set ⊆ Old's; required-ness may only *increase* (optional→required or optional→dropped is narrowing; required→optional is forbidden). New may not introduce a field Old lacked (it would have been illegal under Old). Nested records recurse by the same rule.
- **Maps → records** is valid narrowing: `{x: int, y: int} <: {[str]: int}`.
- **Lists:** covariant element plus length-refinement implication. **Unions:** drop or narrow variants.
- **`require` predicates are re-validated, never implication-checked.** A redefined type inherits Old's `require`s plus any new ones, and the merged value is checked against all of them at validation time. This keeps the subtype check decidable; the cost is that a contradictory `require` is caught when a value exists, not at the type level. This limit is intentional and stated.

### 5.6 Per-type version pinning

A type reference may carry its own `@version`/`@hash`, overriding the version inherited from its `use` for that one slot:

```
use infra/k8s/core @v5.0 as k

schema k.Deployment                        # whole document on v5.0

container.probe: k.Probe @v4.8             # this type pinned back to v4.8 — visible in the diff
```

Pinning is **an override insertion, not a cross-version boundary problem.** There is no special "version-skew check" and no new machinery. The pin overrides which schema validates that subtree, and inserts a value; the value then lives under the rules already defined:

- The inserted value is validated against the **pinned** type (`Probe@v4.8`), exactly as in §5.5's value-level discipline.
- If a parent `require`, a consumer, or the surrounding schema needs a shape the pinned value does not provide (e.g. v5.0 references a field v4.8 lacks), that surfaces as an **ordinary validation error** against the parent — the same error any override can produce. It is not a distinct error class.
- The author owns this exactly as they own any override: an override can produce a value a parent constraint rejects, and that is reported through normal validation, never silently reconciled.

Two properties make pins safe to live with:

1. **Explicit at the use-site and present in the canonical form.** A pin is semantic: two documents differing in a type pin are *different documents* with different content hashes, because they mean different things. The pin is visible where it is written (in the diff a reviewer reads), not hidden in the lockfile. The lockfile records the resolved hash for the pinned type as a distinct entry (`"infra/k8s/core#Probe@v4.8" = "b3:…"`) for reproducibility; the *intent* lives in the document.
2. **Advisory staleness lint.** A pin older than the document's main `use` version is flagged as tech-debt — surfaced, never blocked. This keeps the legitimate *temporary* use (pin a type back while migrating off a breaking change) clean, and makes the illegitimate *permanent* frankenschema state visibly uncomfortable.

---

## 6. L3 — Templated

### 6.1 A file is a function of its params

A `params` block makes a document a parameterized module — a function whose body reads like ordinary config with holes. There is no lambda syntax, no fat arrow, no sigil.

```
use infra/k8s/deploy.schema @b3:7e1f2a as k
schema k.Deployment

params {
  env:     "dev" | "staging" | "prod"
  version: str
}

name:     "api-gateway"
replicas: match env { dev: 1, staging: 2, prod: 6 }
image:    "registry.example.eu/api:${version}"
```

Calling a module is supplying its params:

```
use infra/charts/webapp.chart @b3:9c4e10 as webapp

emit: webapp(
  name:  "api-gateway"
  image: "registry.example.eu/api:1.21.0"
  env:   "prod"
)
```

### 6.2 Functions

Total, non-recursive functions are **definable in schemas** and **callable in documents** (you cannot *define* a function inside a document). They are the principled answer to "short form is sugar for long form" — a normalizing constructor with a defined canonical output, not an overload:

```
type Port = { number: int, name: str }
fn port(n: int): Port = { number: n, name: "http" }

port: port(8443)        # canonical form is the record; the sugar is explicit at the call site
```

### 6.3 Interpolation

Interpolation reuses the universally understood shell/JS shape. It is legal inside `"…"` and `"""…"""`, and disabled inside raw strings.

```
tag:   "$version"                 # bare $name when followed by a non-identifier char
image: "registry.example.eu/api:${version}"
host:  "${name}.svc.local"        # braces when the name abuts more identifier chars
shell: r"""echo "$HOME""""        # raw: $HOME is literal, not interpolated
```

A literal `$` in a non-raw string is `\$`. Templating operates on **values, not text** — interpolation can only produce the typed value of the field it lands in, so it cannot corrupt document structure (no `nindent`, no whitespace-control dashes).

### 6.4 Overloading is not supported — by design

Dispatching on a value's shape would (a) reintroduce inference, (b) break canonical form (which shape is canonical?), and (c) make validation errors a disjunction rather than a fact. The ergonomics people want from overloading are recovered by **union types** (§4.3, honest multi-shape) and **normalizing constructors** (§6.2, sugar with a defined canonical form).

---

## 7. Canonical form

Every document reduces to exactly one normalized representation:

1. Keys sorted (lexicographic by Unicode code point).
2. Units and numbers normalized to canonical form (§4.5; largest exact unit; decimal not float).
3. Defaults materialized; `unset` fields removed.
4. Ordinary `#` comments dropped; `##`/`#!` retained (they are semantic).
5. Author key-ordering discarded (it lives only on the text projection).

The normalized form is encoded as **CBOR** for the wire and hashed with **BLAKE3** to produce the content address (`b3:…`). Two documents differing only in comments, key order, or unit spelling produce identical hashes.

The bijection is over *semantic content*, not bytes: the human text projection preserves comments and author ordering; the canonical form preserves meaning. Round-tripping text → canonical → text is lossless for meaning, lossy for `#` comments and layout (by design — those are properties of the writing, not the value).

---

## 8. Multi-document files

A file is an **ordered, individually-addressed stream of documents**. "One canonical thing per file" applies per *document*; each document is independently content-addressed and the file hash is the merkle root over its members. `---`-separated multi-resource YAML output (e.g. k8s Deployment + Service + ConfigMap) is an **emit projection** of a document stream, not a special syntax.

```
emit: [
  k.Deployment { … },
  k.ConfigMap  { … },
  match expose { true: k.Service { … }, false: unset },   # unset drops the document
]
```

---

## 9. Schema evolution

- A document records the content-hash of the schema it validated against (same pin mechanism as imports).
- When a shared schema tightens to a new hash, existing documents keep validating against *their* pinned schema. Nothing breaks retroactively.
- Upgrading is a deliberate `mangrove migrate` that re-pins and reports violations against the new rules.
- Fields carry `@since(...)`, `@deprecated(...)`, `@removed("use image_ref")` for good migration messages.
- A schema may ship a `migrate` block: a total, content-addressed transform from old shape to new shape, so upgrading many pinned documents is reproducible and reviewable rather than a hand-edit marathon.

---

## 10. Emit projections and the null escape

The model has no null (§2.4). Emit targets may have representational needs the model does not: a few Kubernetes fields are cleared by an explicit JSON `null`. This is handled at the **emit layer**, never in the language:

```
tls: clear()      # sentinel: emits literal `null` in YAML/JSON output only
```

`clear()` is not authorable as a value, never appears in canonical bytes, and never affects the hash. Null is an output-projection artifact, not a model citizen.

---

## 11. Evaluation safety

1. **Pure.** Evaluation has no IO, no clock, no network. A document is a pure function of its inputs.
2. **Hash-verified imports.** The resolver fetches imports separately by content address and verifies the hash *before* evaluation; a mismatch is a hard failure.
3. **Bounded.** Evaluation runs under a fuel + memory budget and aborts loudly past a ceiling. Totality guarantees termination but not cheap termination; the budget is belt-and-suspenders against pathological-but-terminating blowups.
4. **Opaque secrets.** `secret("kv/...")` evaluates to a typed opaque placeholder. It never resolves to a value during evaluation, so a secret cannot enter the canonical bytes or the hash. Resolution happens at *apply* time, outside the evaluator (e.g. External Secrets / OpenBao).

```
DATABASE_URL: secret("kv/api/db-url")    # type-checks as str now; resolves at apply
```

---

## 12. Structured validation errors

Validation failures are structured data, not opaque text — designed to be consumed by humans, CI, and coding agents alike.

```
error: container.port
  got:      70000
  type:     int & >= 1 & <= 65535
  failed:   <= 65535
  message:  "port must be between 1 and 65535"     # from @message
  at:       api.mang:8:9
```

Field path, the constraint violated, the expected type, and any `@message` are always present. Because there is no inference, the expected type is always a single fact, never a disjunction.

---

## 13. Conformance

The specification is an **executable test corpus**, CommonMark-style, not prose alone. An implementation conforms iff it passes the suite. Two vector kinds:

- `(input → canonical-output → hash)` — pins key-sort, number/unit normalization, and CBOR/BLAKE3 output so that a hash computed by one implementation matches another's byte-for-byte.
- `(input → expected-errors)` — pins validation behavior.

Byte-identical canonicalization across implementations is the property the entire content-addressing thesis rests on; it is guaranteed by vectors, not by description.

---

## 14. Readability stance

Bare (no tooling) readability is **better than JSON** — comments, trailing commas, no quote-everything, units, one field per line — and **marginally below YAML** only on airiness. The editor projection (concealing braces into an indented tree) closes that last gap where the plugin runs, with a zero-cost off-switch: turn it off and the braced text is still valid, greppable, and diffable. Because braces are the truth, no whitespace bug can ever be *introduced*; the formatter owns layout.

---

## Appendix A — Grammar sketch (non-normative)

```ebnf
document    = { directive } , { statement } ;
directive   = "#!" , text , newline ;
statement   = binding | typedef | unitdef | fndef | require | spread | listop ;

binding     = key , [ ":" , type ] , [ "=" , value ] ;
typedef     = "type" , name , "=" , type , { annotation } ;
unitdef     = "unit" , name , ":" , primitive , "{" , unit-member { "," unit-member } "}" ;
fndef       = "fn" , name , "(" , [ params ] , ")" , ":" , type , "=" , value ;
spread      = "..." , ref ;
listop      = key , "{" , { "patch" str ":" value | "append" ":" value | "remove" ":" str } , "}" ;
require     = "require" , ":" , predicate , { annotation } ;

type        = union ;
union       = intersection , { "|" , intersection } ;
intersection= atom , { "&" , refinement } ;
atom        = primitive | name | record-type | list-type | map-type | literal ;
refinement  = ( ">=" | "<=" | ">" | "<" ) , number
            | "=~" , string
            | literal ;

value       = scalar | record | list | match | call | "unset" | spread ;
record      = "{" , { binding [ "," ] } , "}" ;
list        = "[" , { value [ "," ] } , "]" ;
match       = "match" , expr , "{" , { case } , "}" ;
case        = ( name | "_" ) , ":" , value ;

annotation  = "@" , name , [ "(" , args , ")" ] ;
comment     = "#" , text | "##" , text ;          (* #! handled as directive *)
scalar      = string | raw-string | text-block | int | decimal | bool | bytes | unit-literal ;
```

This sketch is illustrative; the normative grammar ships with the conformance suite.
