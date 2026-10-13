# 0007. One writer at a time per state

- Status: Accepted
- Date: 2026-10-13

## Decision
Every commit is a conditional PUT on HEAD, so concurrent writers can never both win. For longer operations a writer takes `refs/<state>/lease` (TTL 15 s, renewed every 5 s). Laptop CLI and container CLI use the same path. Replicas poll HEAD (default 30 s) and apply new versions with the atomic generation flip.
Leader push, request forwarding and Kubernetes Lease support are deferred to v0.2.
