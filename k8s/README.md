# Mangrove + Kubernetes

Use Mangrove as the *authoring* layer for Kubernetes manifests: write typed,
content-addressed `.mang`, evaluate to YAML, and feed it to your cluster. No
in-cluster component and no second implementation — everything here is a thin
wrapper around the `mangrove` CLI.

## kubectl plugin

`kubectl-mangrove` makes `kubectl mangrove …` work. Install it by putting the
script on your `$PATH` (alongside the `mangrove` binary):

```sh
install -m 0755 k8s/kubectl-mangrove /usr/local/bin/kubectl-mangrove
kubectl mangrove render examples/k8s-deployment.mang   # → Kubernetes YAML
kubectl mangrove apply  examples/k8s-deployment.mang   # render | kubectl apply -f -
kubectl mangrove diff   examples/k8s-deployment.mang   # render | kubectl diff  -f -
```

Multiple resources: model a Kubernetes `List` (`kind: List`, `items: [ … ]`) —
`kubectl apply -f` accepts it directly.

## KRM function (Kustomize / kpt)

`krm-function.sh` is a KRM *exec/container* function that renders a Mangrove
document into the resource stream. Point a function config at the source:

```yaml
apiVersion: mangrove.dev/v1
kind: MangroveRender
source: |
  type D = { apiVersion: "v1", kind: "ConfigMap", metadata: { name: str }, data: { [str]: str } }
  schema D
  apiVersion: "v1"
  kind: "ConfigMap"
  metadata: { name: "app-config" }
  data: { LOG_LEVEL: "info" }
```

The rendered resource(s) are appended to the `ResourceList` items.

## Container image

```sh
docker build -f k8s/Dockerfile -t mangrove:latest .
```

The image carries `mangrove`, `kubectl-mangrove`, and the KRM function (entrypoint),
so it can serve as a kpt/Kustomize function image or a CI rendering step.

## Typing against the real Kubernetes API

The examples hand-write small schemas. To type real manifests against the live
API, generate Mangrove types from the cluster's OpenAPI spec:

```sh
kubectl get --raw /openapi/v2 > k8s-swagger.json
mangrove gen-openapi k8s-swagger.json --root io.k8s.api.apps.v1.Deployment > k8s-types.mang
```

See the repo root README for the generator's scope and limits (notably: k8s's
recursive schemas, e.g. CRD `JSONSchemaProps`, can't be represented under
Mangrove's no-recursion axiom and are emitted as an opaque type with a warning).
