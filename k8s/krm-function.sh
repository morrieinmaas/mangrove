#!/usr/bin/env bash
# A Kustomize/kpt KRM exec function that renders a Mangrove document into the
# resource stream. It reads a ResourceList on stdin and writes one on stdout,
# appending the resources produced by evaluating the Mangrove source carried in
# the functionConfig:
#
#   apiVersion: mangrove.dev/v1
#   kind: MangroveRender
#   source: |
#     <inline Mangrove document>
#
# Requires `mangrove` and mikefarah `yq` (v4) on PATH — both in the container image.
set -euo pipefail

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/in.yaml"

src="$(yq '.functionConfig.source // ""' "$tmp/in.yaml")"
if [ -z "$src" ]; then
  # nothing to render — pass the stream through unchanged
  cat "$tmp/in.yaml"
  exit 0
fi

printf '%s' "$src" > "$tmp/doc.mang"
mangrove export "$tmp/doc.mang" --to yaml > "$tmp/rendered.yaml"   # set -e aborts if render fails

# Normalise the rendered output into a sequence of resources: a Kubernetes `List`
# is flattened to its items; any other document is a single-element sequence.
# `eval-all` handles a multi-document render; `load()` merges as data (no string
# interpolation into the yq program — avoids breakage/injection).
yq ea '[ (select(.kind == "List") | .items[]), select(.kind != "List") ]' \
  "$tmp/rendered.yaml" > "$tmp/seq.yaml"

yq '.items += load("'"$tmp"'/seq.yaml")' "$tmp/in.yaml"
