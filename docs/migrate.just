# Mangrove migration recipes — copy into your project's justfile, or:
#   import 'path/to/migrate.just'
#
# Assumes the `mangrove` binary is on $PATH. Override `src` / `mang` dirs as needed.

src  := "config"        # where your existing .yaml / .toml live
mang := "mang"          # where generated/authored .mang live

# Bulk-convert every YAML/TOML under `src` into a schemaless .mang under `mang`.
# Run once at the start of a migration, then hand-edit the .mang files.
import-all:
    @mkdir -p {{mang}}
    @find {{src}} \( -name '*.yaml' -o -name '*.yml' -o -name '*.toml' \) -print0 \
      | while IFS= read -r -d '' f; do \
          out="{{mang}}/$(basename "$f" | sed 's/\.[^.]*$/.mang/')"; \
          echo "import $f -> $out"; \
          mangrove import "$f" > "$out"; \
        done

# Generate Mangrove types from the live cluster's OpenAPI (k8s).
# Usage: just gen-k8s-types io.k8s.api.apps.v1.Deployment
gen-k8s-types root:
    kubectl get --raw /openapi/v2 > {{mang}}/k8s-swagger.json
    mangrove gen-openapi {{mang}}/k8s-swagger.json --root {{root}} > {{mang}}/k8s-types.mang

# Validate every .mang against its bound schema. Fails the build on any error.
check:
    @find {{mang}} -name '*.mang' -print0 \
      | while IFS= read -r -d '' f; do echo "check $f"; mangrove check "$f" || exit 1; done

# CI gate: fail if any .mang is not formatted (writes nothing).
fmt-check:
    @find {{mang}} -name '*.mang' -print0 | xargs -0 mangrove fmt --check

# Format every .mang in place.
fmt:
    @find {{mang}} -name '*.mang' -print0 | xargs -0 mangrove fmt

# Render a single .mang document to Kubernetes/other YAML.
# Usage: just render mang/deployment.mang
render file:
    mangrove export {{file}} --to yaml

# Pin namespaced imports into mangrove.lock (run after adding/upgrading a `use`).
update file:
    mangrove update {{file}}

# Assert a .mang still hashes to a recorded value — the migrate-and-verify net.
# Usage: just verify mang/deployment.mang b3:b13a09...
verify file want:
    @got=$(mangrove hash {{file}}); \
      if [ "$got" = "{{want}}" ]; then echo "OK  {{file}} == {{want}}"; \
      else echo "DRIFT {{file}}: got $got want {{want}}"; exit 1; fi
