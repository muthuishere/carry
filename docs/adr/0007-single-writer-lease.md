# 0007. One writer at a time per state

- Status: Accepted (amended 2026-10-13)
- Date: 2026-10-13

## Decision
Every commit is a conditional PUT on HEAD, so concurrent writers can never both win. **In v0.1 correctness relies on HEAD compare-and-swap alone**: a writer that loses re-reads HEAD and re-applies its op.

The lease `refs/<state>/lease` (`{"holder", "expires"}`, TTL 15 s) is taken only by `carry rollback`, so two rollbacks (whole-tree replacements) never interleave; a rollback while another live holder has the lease fails with a conflict. Release is conditional: carry re-reads the lease and only releases it when the holder matches **and** the etag is unchanged since carry wrote it, and it releases by overwriting the record with an already-expired one via If-Match (object_store has no conditional delete), so a lease someone else took over is never touched.

Laptop CLI and container CLI use the same path. Replicas poll HEAD (default 30 s) and apply new versions with the atomic generation flip.
Leader push, request forwarding, lease renewal for long operations and Kubernetes Lease support are deferred to v0.2.
