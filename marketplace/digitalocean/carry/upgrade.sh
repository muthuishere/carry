#!/bin/sh
set -e
helm repo update > /dev/null
STACK="carry"; CHART="carry/carry"; NAMESPACE="carry"
ROOT_DIR=$(git rev-parse --show-toplevel)
helm upgrade "$STACK" "$CHART" --atomic --namespace "$NAMESPACE" --values "$ROOT_DIR/stacks/$STACK/values.yml" --timeout 8m0s
