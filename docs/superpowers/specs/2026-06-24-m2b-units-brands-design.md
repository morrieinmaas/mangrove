# M2b — Units, Brands & the Resolved Canonical Form: Design

> **Status:** Proposed — 2026-06-24
> **Milestone:** M2b (second L1 slice; M2a = typed core, M2c = match/require/annotations/defaults).
> **Normative source:** `mangrove-spec.md` §4.5 (unit types), §4.6 (brands), §7 (canonical form).
> **Goal:** Unit types with declared, discoverable literal suffixes (`512Mi`, `1core`, `0.5btc`), nominal brands, and — the foundational part — **the content address becomes the schema-*resolved* canonical form**, so units normalize into the hash exactly as §7 intends. L0 (schemaless) hashes are unchanged.

This is the milestone that gets the canonical form *right* rather than rushed: it revisits M1's Decision D9 (hash = raw L0 data) and replaces it with §7's actual meaning (hash = resolved canonical value).

---

## 1. The central decision (D12) — canonical form = the resolved value

§7 defines the canonical form as having keys sorted, **units normalized to the largest exact unit**, **defaults materialized**, decimals normalized, comments dropped. Three of those require the schema. So the canonical form — and therefore the content address — is the **fully schema-resolved value**, not the raw surface data. M1 hashed raw data (D9) as scaffolding; M2b corrects this:

- **The content address is `hash(resolve(document, schema))`.** `resolve` produces the normalized value: unit literals → their canonical base integer, keys sorted, decimals normalized. (Defaults materialization lands with M2c; the resolution pass is built here and extended there.)
- **A schemaless L0 document resolves to itself** — there are no units or schema to apply — so every M1 conformance hash is **unchanged**. L0 remains an exact subset. (Verified by the existing L0 corpus still passing.)
- **Units resolve to a plain base integer** in the resolved value. Per §4.5, `512Mi` and `536870912` *are the same value*, so they produce identical canonical bytes and identical hashes. Brand/unit identity is **not** in the data hash — it is meaning, assigned by the schema (§2.1 no-inference), and lives in the schema's own content hash (pinned separately, M3). This is why Option "brand-tagged in the hash" was rejected: it would contradict §4.5 and make data self-typing.

Consequence for the CLI: `mangrove hash <file>` on a **schema-bound** document resolves first (units → ints) then hashes; on a **schemaless** document it behaves exactly as in M1. `mangrove check` validates and, on success, the resolved value is what would be hashed/emitted.

---

## 2. Scope

**In:** `unit Name : int { member = expr, … }` declarations; unit literals (`512Mi`, `1core`, `0.5btc`) — reversing M1 D3 which made them a lex error; `brand` types (`brand int & …`); the **resolution pass** (`resolve(value, type, env) -> ResolvedValue` with units→base int, largest-exact-unit being a *rendering* concern); validation of unit literals (suffix membership, exact-integer base, range, **distinct unit types don't mix**); brand nominal distinctness with auto-construction of bare literals at typed slots; the content-address reframing (D12); `mangrove hash`/`check` updated.

**Out (deferred):**
- `match`, `require`, annotations, **defaults materialization** (`| *value`) → **M2c** (the resolution pass is built here so M2c only adds default-filling).
- Imports / `use` of shared unit/brand types → **M3** (M2b units/brands are file-local); `std/units` ships then.
- Cross-version pins, subtype redefinition → M3.
- The text **formatter** that renders the largest-exact-unit form (`536870912` → `512Mi`) → tooling milestone; M2b computes the canonical *value* (base int), not the pretty text.
- Brand-mixing *between variables/params* (moving an already-branded value into a different brand slot) → needs params, so **L3/M4**; M2b catches mixing only where a literal's suffix isn't a member of the field's unit type.

---

## 3. Unit declarations (§4.5)

```
unit Bytes : int { B = 1, Ki = 1024B, Mi = 1024Ki, Gi = 1024Mi }
unit CPU   : int { m = 1, core = 1000m }
unit Sats  : int { sat = 1, btc = 100_000_000sat }
```

- A `unit` statement (like `type`/`schema`, lexed via the `is_keyword_stmt` discriminator) declares a unit type over `int` with named members. Each member's value is `int` or `<int><member>` (a previously-declared member of the same unit), evaluated to a **base integer** by a small, total, bounded evaluator (multiply a coefficient by an earlier member's base). Forward/self references and unknown member refs error at load.
- The result is a `UnitDef { name, members: Map<String, BigInt> }` (member → base value), stored in the `TypeEnv` alongside types. A unit name is also a usable type (`Bytes` as a field type).

**Decision D13 — member arithmetic is `coefficient × earlier-member` only.** `1024Ki` means `1024 × Ki.base`. No general expressions, no `+`, no forward refs — keeps it total and trivial. (`100_000_000sat`, `1024Mi` all fit.)

---

## 4. Unit literals (§4.5) — reverses M1 D3

```
512Mi      # 512 × Mi.base
1core      # 1 × core.base
0.5btc     # 0.5 × btc.base  → must be an exact integer in the base unit
```

- The **lexer** now emits a unit-literal token `Tok::UnitLit(BigDecimal, String)` = `(mantissa, suffix)` when a number is immediately followed by an identifier. (M1 D3 made this a lex error; M2b lexes it. A number followed by an identifier in a *schemaless* L0 doc still ultimately errors — see D14 — but at the lexer it is now a token, not an error.)
- A `Value::Unit { mantissa: BigDecimal, suffix: String }` variant carries the **unparsed** literal (resolution needs the field's unit type, since suffixes are scoped per unit type and may collide).
- **Resolution** (against a unit type `U`): the suffix must be a member of `U` (else error: *"unknown unit `MB`; valid: B, Ki, Mi, Gi"*); compute `mantissa × member.base`; it **must be an exact integer** (`0.5btc` = 50_000_000 ✓; `0.5sat` → error "not an integer in base unit"); then apply `U`'s refinement (`>= 0` etc.). The resolved value is `BigInt` (a plain integer in the canonical form).

**Decision D14 — a unit literal requires a unit-typed context.** With no schema (pure L0), a `Value::Unit` cannot be resolved (no unit type) → `mangrove hash`/`check` errors ("unit literal requires a schema"). This preserves M1 D3's intent (no bare units at L0) while letting L1 docs use them.

---

## 5. Brands (§4.6)

```
type Satoshis      = brand int & >= 0
type Millisatoshis = brand int & >= 0     # NOT interchangeable with Satoshis
```

- `brand` prefix on a type gives it a **distinct nominal identity** even when structurally identical to another. The `Type` AST gains `Brand { name: String, inner: Box<Type> }` (the name is the brand identity; `inner` the structural type).
- **Auto-construction at known slots:** a field typed `Satoshis` receiving a bare literal `21000` validates as `Satoshis` automatically (no ceremony) — the common case stays clean.
- **Distinctness** is meaningful only when an *already-branded* value moves into a *different* brand's slot. At M2b there are no branded variables/params (those are L3), so the only enforcement available is: a value validates against a brand iff it validates against the brand's `inner` type. The cross-brand-mixing compile error arrives with params (M4). Stated, not hidden.
- Unit types **are** brands (§4.6): two distinct unit types don't mix because a literal's suffix must belong to the field's unit type (a `Bytes` field rejects `1core`).

---

## 6. Validation & resolution (extends M2a)

- `validate` gains arms for unit-typed fields (`Value::Unit` vs a unit type → resolve + range; `Value::Int` vs a unit type → also allowed, treated as a base-unit integer per §4.5; any other kind → error) and `Brand` (validate against `inner`).
- A new `resolve(value, type, env) -> Value` pass produces the canonical resolved value used for hashing: it walks value+type together turning every `Value::Unit`/base-int in a unit field into the canonical `Value::Int(base)`, recursing into records/maps/lists. For non-unit fields it is the identity. A schemaless document (no schema type) resolves to itself.
- Structured errors (§12) extended: unknown-unit error lists the valid members; non-exact-fractional error names the base unit.

---

## 7. Crates touched

- `mangrove-syntax`: lexer `UnitLit` token; parse `unit` declarations and `brand` type prefix; `Value::Unit` variant (in `mangrove-core`), `Type::Brand`, `UnitDef`.
- `mangrove-core`: `Value::Unit { mantissa, suffix }` variant.
- `mangrove-typed`: `UnitDef` resolution + member evaluator in `TypeEnv`; validator arms; the `resolve` pass.
- `mangrove-canonical`: hashing now takes the **resolved** value (the resolve pass lives in `mangrove-typed`; canonical consumes its output). A `Value::Unit` reaching the CBOR encoder unresolved is a bug (resolution must run first) — encoder asserts/errs rather than guessing.
- `mangrove-cli`: `hash` resolves schema-bound docs before hashing; `check` resolves+validates.
- `mangrove-conformance`: L0 unchanged; new L1 unit/brand vectors (both `→ canonical hash` for resolved docs and `→ errors`).

---

## 8. Decisions to confirm

- **D12** — the content address is the schema-**resolved** canonical form (units→base int, keys sorted, decimals normalized; defaults later). Schemaless L0 resolves to itself → M1 hashes unchanged. Revisits/retires M1 D9.
- **D13** — unit member arithmetic is `coefficient × earlier-member` only (total, no general expressions).
- **D14** — a unit literal needs a unit-typed context; unresolved (schemaless) unit literals error (preserving M1 D3's intent).
- **Brand identity is schema-level, never in the data hash** (follows from §4.5 + no-inference); cross-brand-mixing enforcement waits for params (M4).
- Unit normalization to *largest exact unit* is a **text-rendering** concern (the formatter); the canonical *value* is the base integer.
