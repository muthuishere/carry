#!/bin/sh
set -e
helm uninstall carry --namespace carry
kubectl delete ns carry
