# A container that packages the `mangrove` CLI for use as a kubectl plugin base
# or a Kustomize/kpt KRM function. Multi-stage: build the binary, then a slim
# runtime with `kubectl`-friendly tooling.
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p mangrove-cli

FROM debian:stable-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates git yq bash \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/mangrove /usr/local/bin/mangrove
COPY k8s/kubectl-mangrove /usr/local/bin/kubectl-mangrove
COPY k8s/krm-function.sh   /usr/local/bin/krm-function
RUN chmod +x /usr/local/bin/kubectl-mangrove /usr/local/bin/krm-function
# As a KRM function image, the entrypoint reads a ResourceList on stdin.
ENTRYPOINT ["/usr/local/bin/krm-function"]
