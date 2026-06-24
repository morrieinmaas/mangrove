# M1 — L0 Data: Design

> **Status:** Proposed — 2026-06-24
> **Milestone:** M1 of the roadmap (`2026-06-24-mangrove-roadmap-design.md`).
> **Normative source:** `mangrove-spec.md` §3 (L0), §7 (canonical form), §8 (multi-doc), §13 (conformance).
> **Goal:** `mangrove hash file.mang` parses an L0 document, reduces it to canonical form, encodes it as deterministic CBOR, and prints its BLAKE3 content address `b3:<hex>`. The conformance harness runs real `(input → canonical → hash)` vectors.

This pins the L0 grammar subset, the value model, the canonical CBOR encoding, and the hash format precisely enough that the bytes are reproducible. Where the source spec is silent or self-contradictory, this document **makes the decision explicit** (§7 below) — as the reference implementation, our concrete choices realize the spec.

---

## 1. Scope

**In:** L0 lexing (scalars, strings, comments, separators), parsing a document to a value tree, canonical form (§7 steps that apply at L0), a hand-written deterministic CBOR encoder, BLAKE3 hashing, the `mangrove hash` command, and L0 conformance vectors.

**Out (deferred to later milestones):** schema/types, units, brands, `match`, `require` (all L1/M2); interpolation, `params`, `fn` (L3/M4); the text-projection formatter and editor concealment (§14); multi-document streams + merkle-root file hashing (§8 — M1 is one document per file); structured validation errors §12 (M1 emits plain parse errors with positions).

---

## 2. Crates touched

- **New:** `mangrove-syntax` (lexer + parser → `mangrove_core::Value`). deps: `mangrove-core`.
- **New:** `mangrove-cbor` (deterministic canonical CBOR encoder). deps: `mangrove-core`.
- **New:** `mangrove-canonical` (orchestrates: value → canonical CBOR → BLAKE3 → `b3:` string). deps: `mangrove-core`, `mangrove-cbor`, `blake3`.
- **Extend:** `mangrove-core` — add the `Value` model.
- **Extend:** `mangrove-cli` — add the `hash` subcommand.
- **Extend:** `mangrove-conformance` — add `run_vector` (parse → canonical → hash, compare to `.expected`); replace `smoke.expected` placeholder with the real hash.

Workspace deps added: `num-bigint`, `bigdecimal`, `blake3`.

---

## 3. The value model (`mangrove-core`)

L0 has no schema, so every `{ … }` is a **map** (spec §3.2: "At L0 with no schema, `{ … }` is read as a map"). Records (named-field) are an L1 distinction and get a separate variant in M2.

```rust
use num_bigint::BigInt;
use bigdecimal::BigDecimal;
use std::collections::BTreeMap;

/// A canonical Mangrove value. Construction already implies canonical form:
/// map keys are sorted (BTreeMap), numbers are normalized at build time.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(BigInt),               // arbitrary precision
    Decimal(BigDecimal),       // arbitrary precision, normalized (no trailing zeros)
    Str(String),               // UTF-8, escapes already resolved
    Bool(bool),
    Bytes(Vec<u8>),
    List(Vec<Value>),          // ordered, NOT sorted
    Map(BTreeMap<String, Value>), // keys sorted by Unicode code point (= UTF-8 byte order)
}
```

`BTreeMap<String, _>` gives spec §7.1's "lexicographic by Unicode code point" key order for free, because Rust's `String: Ord` is UTF-8-bytewise and UTF-8 preserves code-point order.

`Int` vs `Decimal` is decided by **surface syntax** (no inference, §2.1): a literal with a `.` (or exponent) is `Decimal`; otherwise `Int`. So `3` ≠ `3.0` at L0 — different kinds, different hashes. This is correct and intended.

---

## 4. L0 grammar subset (M1)

```ebnf
document   = { directive } , { binding } ;          (* implicit root map *)
directive  = ( "#!" | "##" ) , text , newline ;     (* lexed & preserved; see §7 on hashing *)
binding    = key , ":" , value , separator ;
key        = bareword | string ;
separator  = newline | "," | end-of-input ;         (* trailing separator legal *)

value      = scalar | list | map ;
map        = "{" , { binding [ "," ] } , "}" ;
list       = "[" , { value [ "," ] } , "]" ;

scalar     = string | raw-string | text-block | int | decimal | bool | bytes ;
int        = [ "-" ] , digit , { digit | "_" } ;
decimal    = [ "-" ] , digit , { digit | "_" } , "." , digit , { digit | "_" }
           | <same> , [ ("e"|"E") , [ "+"|"-" ] , digit , { digit } ] ;
bool       = "true" | "false" ;
bytes      = "b64" , '"' , base64 , '"' ;
string     = '"' , { char | escape } , '"' ;        (* escapes: \" \\ \n \t \r \0 \$ \xNN \u{…} *)
raw-string = "r" , '"' , { any-but-quote } , '"' ;  (* no escapes *)
text-block = '"""' , newline , { line } , indent , '"""' ; (* margin = column of closing """ *)
comment    = "#" , text ;                            (* ordinary: dropped, never hashed *)
```

Notes:
- **Bindings use `:`** (colon), JSON-style: `port: 8443`. (See decision D1, §7.)
- **Top level is a sequence of bindings** = the root map; no outer braces required (matches the `pyproject` example §6.3). (Decision D2.)
- Bareword keys match `[A-Za-z_][A-Za-z0-9_-]*`; any other key must be quoted.
- Underscores are digit separators in numbers and are stripped (`100_000` → `100000`).
- **Unit-suffixed literals** (`512Mi`) are an L1 concept (units come from a schema) → at L0 a number immediately followed by an identifier is an error: *"unit literals require a schema (L1)"*. (Decision D3.)
- **Interpolation** (`${…}`) is L3. At L0 there are no params to resolve, so a non-raw string is taken **literally** (escapes still processed; `\$` → `$`). M1 conformance vectors avoid `${…}`; M4 layers interpolation on without changing the hash of any document that does not use params. (Decision D4.)

---

## 5. Canonical form at L0 (spec §7)

Applying §7's five steps, restricted to what exists at L0:

1. **Keys sorted** — lexicographic by Unicode code point. Free via `BTreeMap`.
2. **Numbers normalized** — `Int` is exact; `Decimal` normalized to strip trailing zeros (`1.20` → `1.2`, `3.140` → `3.14`). Unit normalization (largest exact unit) is N/A at L0 (no units) and lands in M2.
3. **Defaults / `unset`** — N/A at L0 (no schema, no `unset`).
4. **Comments** — ordinary `#` dropped (never hashed) ✓. `##`/`#!` retention in the hash is **deferred to M2** (Decision D5).
5. **Author key-order discarded** — the parser builds straight into a sorted `BTreeMap`, so order is never retained.

---

## 6. Deterministic CBOR encoding (`mangrove-cbor`) — the hand-written core

Encodes a `Value` to canonical CBOR bytes. **Hand-written** so we honor spec §7.1 over RFC 8949's default map ordering (Decision D6). Rules (RFC 8949 §3 wire format, §4.2 determinism, with the §7.1 override):

| `Value`        | CBOR encoding |
|----------------|---------------|
| `Int(n)`, `0 ≤ n < 2^64` | major type 0, shortest-form unsigned |
| `Int(n)`, `-2^64 ≤ n < 0` | major type 1, shortest-form |
| `Int(n)`, outside 64-bit | bignum: tag 2 (≥0) / tag 3 (<0), big-endian minimal bytes, then a byte string |
| `Decimal(d)`   | tag 4 (decimal fraction) → array `[exponent, mantissa]`, where `d = mantissa × 10^exponent`; `mantissa: Int` (bignum-encoded if large), `exponent: Int`; `d` first normalized so `mantissa` is not divisible by 10 (unique form); `0` → `[0, 0]` |
| `Str(s)`       | major type 3, shortest-form length prefix, UTF-8 bytes |
| `Bool(b)`      | `0xf5` (true) / `0xf4` (false) |
| `Bytes(b)`     | major type 2, shortest-form length prefix, raw bytes |
| `List(xs)`     | major type 4, definite length, elements in **insertion order** |
| `Map(m)`       | major type 5, definite length, entries in **key code-point order** (per §7.1; NOT RFC 8949 length-first); each entry = encode(key as `Str`) then encode(value) |

All length/integer headers use the shortest of the 1/2/3/5/9-byte forms (canonical). No indefinite-length items. No floats ever (major type 7 doubles are unreachable — `Decimal` covers all non-integers).

`pub fn encode(value: &Value) -> Vec<u8>`.

---

## 7. Decisions & resolved spec inconsistencies

- **D1 — binding separator is `:`** (not `=`). The source spec is inconsistent: §3.5/§3.6/§6.3 use `key: value`; §4.2 introduces `=` for "value only." L0 (§3, normative for this layer) uses colon exclusively, so M1 uses colon. The `=`-vs-`:` (value-vs-type) reconciliation belongs to M2 (L1), where the type/value distinction actually exists.
- **D2 — top-level document is a binding sequence** = the root map (no required outer braces), matching the `pyproject` example (§6.3). A single brace-wrapped value at top level is *not* a valid M1 document.
- **D3 — unit literals error at L0** (units need a schema). Deferred to M2.
- **D4 — `${…}` is literal at L0** (no interpolation engine until M4); `\$` escapes to `$`.
- **D5 — `##`/`#!` are lexed and preserved on the AST but NOT yet folded into the hash.** §7.4/§3.6 say they are semantic and hashed, but their canonical *representation* is bound to schema semantics (`##` → `@doc` on a schema field; `#!schema` → a binding) that do not exist until L1. M1 hashes the **data value** only. Upgrade path: M2 defines their hashed encoding and the hash of any document using them changes then — documents using neither are unaffected. Stated, not hidden.
- **D6 — map keys sorted by Unicode code point (§7.1), overriding RFC 8949's length-first map ordering.** This is the explicit reason the encoder is hand-written, not delegated to a CBOR crate.

---

## 8. Hash format (`mangrove-canonical`)

```rust
pub fn hash(value: &Value) -> String        // "b3:" + 64 lowercase hex chars
pub fn canonical_cbor(value: &Value) -> Vec<u8>
```

`hash` = `"b3:"` + hex(BLAKE3-256(`canonical_cbor(value)`)). BLAKE3 default 32-byte output → 64 hex chars. (Spec examples show truncated `b3:7e1f2a9c…`; we emit the full digest.)

---

## 9. CLI

`mangrove hash <file.mang>` → reads the file, parses, canonicalizes, prints `b3:<hex>\n`, exit 0. Parse error → message with `file:line:col` to stderr, exit 1. The existing `--version` is unchanged.

---

## 10. Conformance corpus (spec §13)

`mangrove-conformance` gains:

```rust
pub fn run_vector(input: &Path, expected: &Path)  // parse → hash; assert == expected.trim()
```

The `corpus.rs` test iterates `vector_pairs(L0_CORPUS)` and calls `run_vector` on each. `smoke.expected`'s `pending-m1` placeholder is replaced with the real computed `b3:` hash. Vectors added to cover: small + bignum ints, negative ints, decimals (incl. trailing-zero normalization), plain/raw/text-block strings with escapes, bool, bytes, empty/nested lists and maps, key-sort ordering, ordinary-comment dropping. Each is `name.mang` + `name.expected` (the hash on one line).

Authoring rule: an `.expected` hash is generated by running the implementation once and **eyeballing the input for correctness**, then committed; thereafter it is a regression pin. (No second implementation exists yet to cross-check, per the roadmap.)

---

## 11. Testing strategy

- **Unit tests** per crate: lexer (token stream for each scalar form, comments, separators), parser (each value shape, errors, key sort), `mangrove-cbor` (byte-exact vectors for each `Value` arm — e.g. `Int(0)` → `0x00`, `Int(23)` → `0x17`, `Int(24)` → `0x18 0x18`, `Bool(true)` → `0xf5`, a bignum, a decimal `[exp,mantissa]`, map key ordering), `mangrove-canonical` (known value → known `b3:` hash).
- **Conformance vectors** as in §10.
- All TDD: failing test first. Gate: `just ci` green.
