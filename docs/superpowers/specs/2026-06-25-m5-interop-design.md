# M5 — Interop: real-world fixtures + format converters (design)

**Goal:** prove Mangrove on real configuration and bridge it to the formats teams already have. Two deliverables: (1) end-to-end fixtures — a Kubernetes Deployment and a `pyproject`-style config written in Mangrove, schema-checked and hashed in CI; (2) YAML↔Mangrove and TOML↔Mangrove converters with round-trip e2e tests.

## Why now

L0–L3 are complete and reviewed. The fixtures are the first exercise of the whole stack (types + refinements + units + composition + templating) on configuration of realistic size and shape; they also become regression anchors. The converters make Mangrove adoptable incrementally — import an existing YAML/TOML file, get a typed, content-addressed document; export back for tools that still consume YAML/TOML.

## Slices

### M5a — real-world fixtures (no new code; tests + example docs)

- A Kubernetes Deployment in Mangrove: a `Deployment` schema (apiVersion/kind literals, metadata, spec with replicas refinement, container list with `@key(name)`, resource units `512Mi`/`0.5` CPU via a unit type), plus a concrete document. e2e: `check` passes; `hash` is stable (golden hash asserted); an out-of-range replica fails.
- A `pyproject`-style document: `[project]` fields (name, version, dependencies list), `[build-system]`, optional-dependency groups. e2e: `check` passes; `hash` stable; a malformed version fails.
- These live under `examples/` (committed) and drive `crates/mangrove-cli/tests/` e2e cases. No library changes — purely exercising the existing pipeline. If a fixture exposes a real gap (a spec feature that does not round-trip), file it as a finding, do not paper over it.

### M5b — YAML ↔ Mangrove

- **New crate `mangrove-convert`** (dep: `serde_yaml` or `serde_yaml_ng`, `serde_json` as the intermediate, plus core/syntax). Two directions over `mangrove_core::Value`:
  - **import** `yaml → Value`: map YAML scalars to `Int`/`Decimal`/`Str`/`Bool`; sequences→`List`; mappings→`Map` (string keys only — a non-string YAML key is an error, matching Mangrove's string-key model). **YAML `null` is rejected** (Mangrove axiom §2.4: no null) — the converter errors with the path, rather than inventing a value. Numbers parse to exact `BigInt`/`BigDecimal` (no f64 round-trip — go through the YAML number's source text). Then render `Value` as Mangrove document text.
  - **export** `Value → yaml`: the inverse; total because `Value` (post-eval) has no markers and no null.
- **D42 — import produces L0 data only.** A converted document has no schema, types, units, or templating — it is plain data. The user adds a schema afterward. This keeps the converter a pure data bridge and avoids guessing types (no inference, §2).
- **D43 — round-trip identity is at the VALUE level, not the byte level.** `yaml → Value → yaml` need not be byte-identical (comments, key order, anchors are lost), but `yaml → Value` then `Value → mangrove → hash` must be stable, and `mangrove → Value → yaml → Value` must equal the original `Value`. The content hash is the round-trip invariant, not the text.
- CLI: `mangrove import <file.yaml>` (prints Mangrove text), `mangrove export <file.mang> --to yaml` (prints YAML of the evaluated value).
- e2e: a YAML fixture imports to Mangrove, hashes stably; export re-emits YAML whose re-import equals the original value; `null` → clean error; a float like `0.1` survives as an exact decimal (no `0.1000000000000000055` f64 artifact).

### M5c — TOML ↔ Mangrove

- Same crate, `toml` dep (already in the workspace). `toml → Value` and back. TOML specifics:
  - TOML tables/inline-tables → `Map`; arrays → `List`; integers → `Int`; floats → `Decimal` (exact via source text); booleans → `Bool`; strings → `Str`.
  - **TOML datetimes** have no Mangrove scalar: **D44** — import maps an offset/local datetime to a `Str` carrying the RFC 3339 text (lossy-but-honest; documented), or errors if a stricter mode is wanted. Pick string-mapping for ergonomics; note it.
  - TOML has no null, so no rejection needed there.
- CLI: `mangrove import <file.toml>` (dispatch by extension or `--from toml`), `export --to toml`.
- e2e: the pyproject fixture round-trips TOML→Mangrove→value; a datetime maps to a string and back; nested tables preserve structure and hash.

## Cross-cutting decisions

- **D45 — exact numbers across the bridge.** Never route a number through `f64`. Parse YAML/TOML numbers from their source representation into `BigInt`/`BigDecimal`; serialize `BigInt`/`BigDecimal` back via their own `to_string`. This preserves Mangrove's arbitrary-precision guarantee and keeps hashes stable.
- **D46 — converters operate on the EVALUATED value for export.** `export` runs the full pipeline (compose → eval → resolve) and serializes the canonical value, so exporting a templated Mangrove document yields concrete YAML/TOML. Import is the inverse and yields plain L0 data (D42).
- **No null, ever.** Both importers reject null/None; both exporters never emit null (the value model has no absence to emit — `unset` removed keys during composition).

## Testing posture

Hermetic, golden-hash anchored. Each fixture asserts a stable `b3:` hash (regenerated only on an intentional change). Converter round-trips assert value-level identity (D43) and exact-number preservation (D45). Adversarial: null, non-string keys, huge numbers, deeply nested structures (depth bounds), datetime.

## Out of scope

- YAML anchors/aliases/merge-keys, multi-document streams → flatten or error, not resolve.
- Comments and formatting preservation (D43: value-level, not text-level).
- Schema inference from data (§2: no inference — import is schemaless).
