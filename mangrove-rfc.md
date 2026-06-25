# RFC: Mangrove — A Verifiable, Layered Configuration Language

> **Name:** Mangrove
> **Status:** Request for Comments — Draft 0.1
> **Companion:** `mangrove-spec.md` (normative reference)
> **Author intent:** full L0–Ln implementation; this RFC argues the design, the spec defines it

---

## Abstract

Mangrove is a typed configuration language with a single content-addressable canonical form, built on one principle: **no surface type inference — the schema is the sole authority on what a value means.** From that one decision a stable canonical form follows, and from a stable canonical form follow hashing, signing, reproducible builds, semantic diff, and unambiguous machine consumption. Mangrove is structured as four concentric layers (Data → Typed → Composed → Templated) so that a document costs, in concepts a reader must hold, only the power it actually uses. It targets the full span from "a better TOML" to "a better Helm" without changing languages.

---

## 1. Motivation

### 1.1 The lineage, stated correctly

TOML descends from INI, not YAML — Tom Preston-Werner's pitch was "INI but actually specified." It is excellent when flat and genuinely painful once nested: `[[servers.backend.replicas]]` forces a reader to reassemble a tree from flat section labels. YAML reads beautifully because indentation renders structure as a visual tree — but its readability is **front-loaded**: lovely to skim, treacherous to edit. Significant whitespace is invisible, so a misaligned key silently reparents, and copy-paste corrupts levels with no error. JSON has the opposite problem: an unambiguous canonical-ish form that is miserable to author and cannot carry a comment.

The deeper issue underneath all three is **surface type inference**. YAML guessing that `NO` is `false`, `1.20` is `1.2`, and `22:22` is a sexagesimal number is not a quirk — it is the root cause of an entire bug class, and it is precisely what makes YAML impossible to canonicalize, hash, or sign.

### 1.2 The space has no single optimum

"Closer to perfect" assumes perfect is a point. It is not. The requirements are mutually hostile: human-writability wants terseness and no quotes; machine-unambiguity wants explicit delimiters and no inference; deep nesting wants indentation or braces; composition wants variables and imports — at which point you have stopped being a data format and become a programming language. Every format is a *coordinate* in that space. xkcd 927 ("now there are 15 competing standards") is the law of the domain, and a fifteenth flat text format does not escape it.

### 1.3 Where the serious work actually went

The people who chased "closer to perfect" did not build another flat format. They concluded that the boring wire format should stay boring and machine-consumed, and that **correctness belongs in a typed authoring layer that compiles down to it** — CUE, Dhall, Pkl, Nickel, and at the opposite extreme NestedText/StrictYAML (no inference, types from a schema). Mangrove is an entry in that family. Its contribution is not "yet another typed config language" but a specific, internally coherent set of choices — no-inference as the load-bearing axiom, a single canonical form, units-as-brands, and a layer model that bounds the mental cost — that, taken together, are not occupied by any existing member.

---

## 2. Goals and non-goals

### 2.1 Goals

- **Eliminate inference bugs** by making the schema the only source of type.
- **One canonical form** so configs can be hashed, signed, cached, reproduced, and diffed semantically.
- **Bounded mental cost** via layers: simple configs require only simple concepts.
- **Honest numbers and units** — arbitrary precision, no float, declared discoverable unit suffixes.
- **Value-layer templating** so interpolation cannot corrupt document structure.
- **FOSS-first, git-native** — no proprietary server, no central registry; plain git tags and content hashes.

### 2.2 Non-goals (stated honestly, not buried)

- **Not a zero-tooling YAML replacement on airiness.** Mangrove's bare (no-plugin) form is better than JSON and marginally below YAML in airiness. It does not claim to beat YAML for skim-readability everywhere a plugin is absent. It claims something narrower and true: adequate-everywhere bare readability, excellent readability where the editor projection runs, and *no possible whitespace bug* because braces are the truth.
- **Not "lightweight" in the TOML sense.** The full language (L0–L3) is comparable in ambition to CUE/Dhall/Pkl. The honest claim is **a strictly smaller mental model per tier of use**, not fewer total features. A `pyproject` author touches only L0/L1 and never loads composition or templating semantics.
- **Not defensible by "agent-native" framing.** Structured errors and no-inference help coding agents, but any typed language with good diagnostics offers that. The load-bearing, structural property is **verifiability** (canonical form + content addressing); agent-friendliness is a corollary, not a moat. The RFC does not pitch it as one.
- **Not Turing-complete.** All computation is total and decidable. No recursion, no user-defined general functions. This is a deliberate ceiling.

---

## 3. Design principles and the decisions they force

| Principle | Consequence |
|-----------|-------------|
| No inference | Schema is sole type authority; canonical form becomes possible; errors are single facts, never disjunctions |
| Braces are truth, whitespace cosmetic | Tabs-vs-spaces footgun cannot exist; formatter owns layout; editor projection is pure gravy with an off-switch |
| One canonical form | BLAKE3 content address; semantic diff; signing; reproducible builds |
| No null | Two states (present/absent), not three; whole `null`/`~`/empty bug class removed |
| Totality | Templates and predicates always terminate; evaluation is safe to run on imported, content-addressed code |
| Explicit over implicit | Composition is one rule + one named exception; overloading rejected; merges never inferred from shape |

The mark of the design holding together is that nearly every downstream choice *derives* from these rather than being bolted on: override, spread, the null ban, the comment trichotomy, the rejection of overloading, and the units-as-brands unification all fall out of "no inference + one canonical form."

---

## 4. Key decisions, with rationale

### 4.1 The layer model is the keystone

Defining Mangrove as L0 (Data) → L1 (Typed) → L2 (Composed) → L3 (Templated) is what makes "small when simple, powerful when needed" true rather than aspirational. The total spec is large; the spec you must *understand* is bounded by the layer you work in. CUE cannot make this claim — unification is load-bearing even for trivial configs, so the hard model is always present. This is the precise, defensible form of "simpler than CUE": **not fewer features, a smaller mental model at each tier.** It also dissolves the "personal tool vs. would-be standard" fork — that becomes "which layers an implementation targets," over one language with one canonical form.

### 4.2 Braces as truth, indentation as render

The significant-whitespace debate is reframed, not taken a side in. Canonical truth is brace-delimited and whitespace-insignificant, so the editing footgun is structurally impossible. The YAML-look is a **render** (the `render-markdown.nvim` model: conceal the brace noise over the *same* text buffer). Critically this is **not** projectional editing (MPS/Unison): the text never stops being the truth, so `grep`, `git diff`, `git blame`, and plain-editor review all keep working. Turn the plugin off and you lose airiness, nothing else. That off-switch is the line between this approach and the projectional-editing graveyard.

### 4.3 Value-layer templating, never text

Helm is hated because it templates *text*: `{{ .Values.x }}` concatenates strings before parsing, so output is not guaranteed to be valid YAML — hence `nindent`, `{{- … -}}` whitespace control, and runtime-only validation. Mangrove templates *values*: interpolation can only produce the typed value of the field it lands in, so it physically cannot break structure, and the result is type-checked at build time. `match` replaces `if/else` and is exhaustiveness-checked; a missing environment is a compile error, not a silent gap.

### 4.4 Units are brands for numerics

`256Mi` and `0.5btc` are the strongest single idea here. Unifying unit literals with newtype safety means: the suffix set is declared by the type (so it is discoverable and autocompleted — nothing to guess, and `256MB` fails loud), values canonicalize deterministically to the largest exact unit, fractional literals are legal only when exact, and distinct unit types cannot mix (a CPU value cannot land in a memory field). It solves a real daily footgun in both k8s quantities and money arithmetic with one mechanism.

### 4.5 Composition collapsed to one rule + one exception

Earlier drafts had four composition mechanisms and a double-duty `&`. The final design collapses them: spread and scalar-override are the *same* "later wins, records deep-merge" rule; value-level "must agree" folds into `require`, leaving **`&` type-only**; and lists are the single irreducible exception (replace by default; `@key` opts into named `patch`/`append`/`remove`). A user learns two ideas and one exception, and every list mutation is a named, greppable verb rather than a structural implication — which is exactly the complexity Kustomize's strategic-merge-patch never escaped.

### 4.6 Override and redefine yes; overload no

Override (values, last-wins) and redefine (types, **subtype-only** narrowing) are both supported and both safe — redefinition is constrained to `New <: Old` and is structural/covariant/recursive, with `require` predicates re-validated rather than implication-checked (the latter is undecidable; this limit is stated, not hidden). Overloading is rejected because shape-dispatch *is* inference, breaks canonical form, and turns errors into disjunctions. What people want from overloading is recovered by **union types** (honest multi-shape) and **normalizing constructors** (`fn`, sugar with a defined canonical form).

**Per-type version pinning is an override insertion, not new machinery.** A type reference can carry its own `@version` (`container.probe: k.Probe@v4.8` inside a `v5.0` document), and the temptation is to model this as a "cross-version boundary" needing a special skew check. It is not. The pin overrides which schema validates that subtree and inserts a value; the value is then validated against the pinned type under the *existing* value-level rules. If a parent constraint needs a shape the pinned value cannot provide, that is an ordinary validation error against the parent — the same error any override can produce — not a distinct error class. The two properties that keep it safe are that the pin is **explicit at the use-site and part of the canonical form** (visible in the diff, not hidden in the lockfile) and that an **advisory staleness lint** flags pins older than the document's main version, so the legitimate temporary use stays clean and the permanent-frankenschema state is visibly uncomfortable.

### 4.7 Versioning is git — and identity, location, and auth are separated

Imports resolve via plain git, but Mangrove deliberately *departs* from the Go-modules model on one point that matters in practice: the import string does not contain the network location. Go conflates identity and location in one string, which is exactly why private repos need `GOPRIVATE` globs, moving a repo needs `replace` directives, and "works on my machine, fails in CI" is a recurring auth problem. Mangrove splits the concern across three places: the **document** carries identity + intent (`use infra/k8s/core @v5.0`), a committed **`mangrove.lock`** carries the pin (tag → content hash, for reproducibility), and a non-committed **`resolvers.toml`** carries location + auth (which remote a namespace lives at, using git's own credential machinery). Private repos then need no special-casing, mirroring or moving a dependency is a one-line resolver change with unchanged hashes, credentials never touch committed artifacts, and a repo with private dependencies can be open-sourced without leaking where they live. Integrity is location-independent: fetched bytes are verified against the lockfile hash before evaluation, so the supply-chain guarantee of a checksum database is obtained from the committed lockfile alone — no checksum-database *server* required, no central registry, no proprietary forge.

---

## 5. Comparison to prior art

| | Inference | Canonical form | Composition model | Computation | Mental cost (simple case) |
|---|---|---|---|---|---|
| **YAML** | aggressive (the bug source) | none | anchors/merge keys (fragile) | none | low to skim, high to edit safely |
| **TOML** | mild | none | none | none | low (flat), high (nested) |
| **JSON** | none | near-canonical | none | none | medium; no comments |
| **CUE** | none | yes | unification (powerful, hard to reason about) | constraints | high even when simple |
| **Dhall** | none | yes (+ semantic import hashing) | explicit imports + functions | total functional language | high (it is a language) |
| **Pkl** | typed | generates outputs | classes/inheritance | scripting | medium-high |
| **NestedText / StrictYAML** | none | no | none | none | very low; types only via external schema |
| **Mangrove** | **none** | **yes (BLAKE3/CBOR)** | **one rule + named list ops** | **total, non-recursive** | **bounded per layer** |

Mangrove's distinguishing position: CUE's rigor *without* unification (composition is explicit and last-wins, not lattice-derived), Dhall's canonical-form-and-hashing *without* being a full functional language, NestedText's no-inference *with* first-class types and a canonical form. Whether that position dethrones YAML is a distribution-and-tooling question (see §7), not a design one — and the RFC does not pretend otherwise.

---

## 6. Worked examples

### 6.1 Kubernetes — schema + templated instance

Schema (L1):

```
# deploy.schema.mang
use std/units { Bytes, CPU, Duration }

type Name  = str & =~ "^[a-z][a-z0-9-]{0,62}$"   @doc("RFC 1123 label")
type Image = str & =~ ".+:.+"   @message("image must be repo:tag — no untagged / :latest")
type Port  = int & >= 1 & <= 65535

type Container = {
  name:   Name
  image:  Image
  ports:  [ Port ]
  cpu:    { request: CPU,   limit: CPU }
  memory: { request: Bytes, limit: Bytes }
  env:    { [str]: str }
  require: cpu.limit    >= cpu.request      @message("cpu limit < request")
  require: memory.limit >= memory.request   @message("mem limit < request")
}

type Deployment = {
  name:       Name
  namespace:  str | *"default"
  replicas:   int & >= 1 & <= 100 | *1
  containers: [ Container ]   @key(name)
  require:    containers != []   @message("need >= 1 container")
}
```

Instance (L3 — templated):

```
# api.mang
use infra/k8s/deploy.schema @b3:7e1f2a as k
schema k.Deployment

params {
  env:     "dev" | "staging" | "prod"
  version: str
}

name:      "api-gateway"
namespace: match env { prod: "production", _: "staging" }
replicas:  match env { dev: 1, staging: 2, prod: 6 }

containers: [
  {
    name:  "api"
    image: "registry.example.eu/api:${version}"
    ports: [ 8443 ]
    cpu:    { request: 250m,  limit: 1core }
    memory: { request: 256Mi, limit: 512Mi }
    env: {
      LOG_LEVEL:    match env { dev: "debug", _: "warn" }
      DATABASE_URL: secret("kv/api/db-url")
    }
  },
]
```

`mangrove build api.mang --env prod` type-checks `replicas: 6` against the bound, enforces `cpu.limit >= request`, rejects an untagged image with the authored `@message`, keeps `256Mi` as a typed `Bytes` (un-mixable with CPU), keeps the secret opaque (never in the hash), and emits ordinary k8s YAML.

### 6.2 A Helm chart — the templating showcase

A chart is a parameterized module whose `emit` is a document stream. `unset` in a `match` arm drops a resource.

```
# webapp.chart.mang
use infra/k8s/core @b3:7e1f2a as k

params {
  name:     k.Name
  image:    k.Image
  env:      "dev" | "staging" | "prod"
  replicas: int & >= 1     | *match env { prod: 3, _: 1 }
  port:     k.Port         | *8080
  settings: { [str]: str } | *{}
  expose:   bool           | *false
}

emit: [
  k.Deployment {
    name:     name
    replicas: replicas
    containers: [{
      name:   name
      image:  image
      ports:  [ port ]
      cpu:    { request: 100m,  limit: 500m  }
      memory: { request: 128Mi, limit: 256Mi }
      env:    { ...settings, APP_ENV: env }
    }]
  },

  k.ConfigMap { name: "${name}-config", data: settings },

  match expose {
    true: k.Service {
      name:     name
      selector: { app: name }
      ports:    [{ port: port, target: port }]
    }
    false: unset
  },
]
```

"Installing the chart" is calling the module with values:

```
# api.prod.mang
use infra/charts/webapp.chart @b3:9c4e10 as webapp

emit: webapp(
  name:     "api-gateway"
  image:    "registry.example.eu/api:1.21.0"
  env:      "prod"                              # replicas defaults to 3
  port:     8443
  expose:   true
  settings: { LOG_LEVEL: "warn", REGION: "eu-west" }
)
```

Every Helm pain — unvalidated `{{ .Values.replicas }}`, `{{- if -}}` whitespace dashes, `toYaml | nindent 8` arithmetic — is gone, because nothing is templated as text. `expose: false` makes the Service *structurally absent*; `settings` flows in as a typed map; `replicas: 999` is a build error.

### 6.3 `pyproject` — proof the simple case stays simple

Templating is opt-in; with nothing to compute, Mangrove is as quiet as TOML, and fixes TOML's two real pains (array-of-tables, dotted section reassembly).

```
# pyproject.mang
use gh/pypa/pyproject-schema @b3:4d2f88 as py
schema py.Project

build-system: {
  requires:      [ "hatchling >= 1.25" ]
  build-backend: "hatchling.build"
}

project: {
  name:            "forgitry"
  version:         "0.4.2"
  description:     "EU-sovereign git forge"
  requires-python: ">= 3.12"
  license:         "AGPL-3.0-or-later"

  authors: [
    { name: "Jeff", email: "jeff@example.eu" },
  ]

  dependencies: [
    "httpx >= 0.27",
    "pydantic >= 2.7",
  ]

  optional-dependencies: {
    dev:  [ "pytest >= 8.2", "ruff >= 0.5" ]
    docs: [ "mkdocs-material >= 9.5" ]
  }

  scripts: { forgitry: "forgitry.cli:main" }
}

tool: {
  ruff:   { line-length: 100, lint: { select: [ "E", "F", "I" ], ignore: [ "E501" ] } }
  pytest: { ini_options: { testpaths: [ "tests" ], addopts: "-ra" } }
}
```

`authors` is a clean inline list of records — no `[[project.authors]]`. `tool.ruff.lint` nests as ordinary records — no dotted-header reassembly. `requires-python: ">= 3.12"` stays the exact string (no version-token coercion). With `schema py.Project`, a typo like `dependancies` is a write-time error, not a silently ignored key.

---

## 7. Known limitations and risks

1. **Tooling-conditional airiness.** Bare form is better than JSON, marginally below YAML. The full YAML feel requires the editor projection. This is the largest adoption liability and it is structural; it is mitigated (not eliminated) by the off-switch and by the fact that the brace density is lowest at L0/L1, exactly where bare review matters most.
2. **`require` subtyping is re-validated, not proven.** Implication between arbitrary predicates is undecidable, so a contradictory `require` in a narrowing is caught when a value exists, not at the type level. Deliberate trade for a decidable checker.
3. **Regex refinement subtyping is PSPACE.** Decidable but not cheap; the one non-trivial scalar subtype case.
4. **Self-hosting needs axioms.** The type-of-types stands on enumerated L0 primitives (`int`, `decimal`, `str`, `bool`, `bytes`, record, list, map). These are a floor; user-defined types extend the set freely.
5. **Verifiability is the moat, not agents.** Stated in §2.2; repeated here so it is not quietly re-smuggled into the pitch.

---

## 8. Adoption path

Because Mangrove is one language across layers, adoption is incremental over the *same* canonical form:

1. **L0/L1 first** — replace a `pyproject` or service config; gain typing and honest numbers with near-TOML simplicity.
2. **L2** — introduce `use` + `mangrove.lock` + overlays; replace a Kustomize layer.
3. **L3** — introduce `params` + `emit`; replace a Helm chart.
4. **Editor projection** — add the conceal/render plugin once daily editing volume justifies it; never required for correctness or review.
5. **Conformance suite** — the gate that makes a second implementation (Go forge, Rust client, etc.) produce byte-identical hashes.

A reader who adopts only L0–L1 has a TOML/JSON replacement; one who adopts L3 has a Helm replacement; both sit on the identical canonical form, so a config can graduate between tiers without migration.

---

## 9. Open questions for reviewers

- Is the editor-projection off-switch a sufficient answer to tooling-conditional readability, or does the bare brace form need further softening (e.g. optional significant-newline sugar that the formatter still normalizes)?
- Should `fn` (total constructors) be allowed only in schemas, or also in a constrained document scope?
- Is merkle-root-over-documents the right file-hash semantics, or should a file be hash-opaque and only its documents addressable?
- Does the `@key` list-operation block cover real Kustomize migration cases, or are there merge patterns it cannot express without a fifth verb?
- Does the resolver split (identity in the document, location+auth in a non-committed `resolvers.toml`) impose acceptable onboarding cost for a fresh clone with private dependencies, and is a generated `resolvers.toml.example` enough to make that step discoverable?
- Per-type pinning is treated as a plain override insertion (no special boundary check; parent-constraint mismatches surface as ordinary validation errors). Is the advisory staleness lint the right pressure against permanent frankenschema pinning, or should a long-stale pin be a hard CI failure under an opt-in policy?

Comments welcome against the companion spec (`mangrove-spec.md`), which carries the normative detail for every section referenced here.
