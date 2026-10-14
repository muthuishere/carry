#!/bin/sh
set -e
helm repo add carry https://muthuishere.github.io/carry
helm repo update > /dev/null
STACK="carry"
CHART="carry/carry"
CHART_VERSION="0.1.1"
NAMESPACE="carry"
if [ -z "${MP_KUBERNETES}" ]; then
  ROOT_DIR=$(git rev-parse --show-toplevel)
  values="$ROOT_DIR/stacks/$STACK/values.yml"
else
  values="https://raw.githubusercontent.com/digitalocean/marketplace-kubernetes/master/stacks/$STACK/values.yml"
fi
helm upgrade "$STACK" "$CHART" --atomic --create-namespace --install --timeout 8m0s \
  --namespace "$NAMESPACE" --values "$values" --version "$CHART_VERSION"
