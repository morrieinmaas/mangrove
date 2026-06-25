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
# Requires `mangrove` and `yq` on PATH (both present in the container image).
set -euo pipefail

input="$(cat)"
src="$(printf '%s' "$input" | yq e '.functionConfig.source // ""' -)"
if [ -z "$src" ]; then
  # nothing to render — pass the stream through unchanged
  printf '%s' "$input"
  exit 0
fi

tmp="$(mktemp -t mangrove-krm.XXXXXX.mang)"
trap 'rm -f "$tmp"' EXIT
printf '%s' "$src" > "$tmp"
rendered="$(mangrove export "$tmp" --to yaml)"

# Append the rendered resource(s) to the ResourceList's items. A Mangrove `List`
# (kind: List) is flattened into individual items; any other doc is one item.
printf '%s' "$input" | yq e ".items += [$(printf '%s' "$rendered" | yq e -o=json '
  if .kind == "List" then .items[] else . end' - | yq e -o=json -)]" -
