# carry: how it makes money

Research date: 2026-10-14. Owner entity: Deemwar (India). Company target: ₹15L/month across all products (company-research `03-distribution.md`, `08-revenue-math.md`).
Every claim is tagged **[verified]** (checked against a primary source this session), **[secondary]** (aggregator or press, not the vendor's own page) or **[unverified]** (from memory, check before relying on it).
Research method: web search only. No chrome-agent browsing was done for this pass, so all vendor pricing pages still need a direct check.

## TL;DR

1. The binary stays Apache-2.0 and free. Money comes from **carry Cloud**, a hosted control plane: web UI for delete/rename/audit, team RBAC, SSO, KMS-wrapped keys, a fleet dashboard and a curated signed tool store. That matches the scope decision already made: agents only add and update, and delete/rename go through a separate admin CLI/web.
2. Fastest first dollar: a **Lemon Squeezy or Paddle** checkout (merchant of record, so they handle global sales tax) for a "carry Team" self-serve plan plus a fixed-price **support/setup contract**. Both can be live in a week or two.
3. Marketplaces come later and are mostly for enterprise procurement. **Trap:** an India-based AWS Marketplace seller can only sell to buyers in India **[verified]**, and container listings pay **20%**, not 3% **[verified]**. To sell globally on AWS you need a non-India seller entity (for example a US LLC), or a SaaS listing.

## 1. Paid marketplace channels

| Marketplace | Fee | Fit for carry | Notes |
|---|---|---|---|
| AWS Marketplace, SaaS public offer | 3% **[verified]** | Best fit once carry Cloud exists | Private offers: 3% under $1M TCV, 2% for $1M–10M, 1.5% for $10M+, 1.5% on renewals; channel-partner private offers (CPPO) add 0.5% **[verified]** |
| AWS Marketplace, container/AMI public offer | **20%** **[verified]** | Poor: this is what "list the image" would cost | Free container listings are allowed; use one only as a discovery funnel |
| AWS India-based seller | GST applied, auto-deducted; paid in INR to an Indian bank account **[verified]** | **Can only sell to buyers in India** **[verified]** | Launched Nov 2025. For global buyers you need a seller entity in another eligible jurisdiction |
| Google Cloud Marketplace | 3% standard; 2% for $1M–10M and 1.5% for $10M+ on private offers **[secondary: Clazar 2026]** | SaaS listing via a Pub/Sub entitlement integration **[unverified]** | Free to list **[secondary]** |
| Azure Marketplace | 3% flat **[secondary]** | SaaS offer + fulfillment API; Azure Kubernetes Service (AKS) container offers exist **[unverified]** | Seller entity rules need checking in Partner Center **[unverified]** |
| Oracle Cloud Marketplace | Paid listings need Oracle partner (OPN) membership **[unverified]** | Low priority | — |
| DigitalOcean Marketplace | Paid add-ons program exists; terms not found **[unverified]** | Mid: indie/SMB audience | One-click apps are free and work as a funnel |
| Red Hat Marketplace / OpenShift certified operator | Red Hat Marketplace was folded into AWS/Azure-based channels **[unverified]** | Only as an "OpenShift certified" badge | Certification costs engineering time, not money **[unverified]** |

Payout mechanics **[unverified unless noted]**: AWS pays monthly in arrears once the buyer has paid, via disbursement to a registered bank account (INR for India sellers **[verified]**). The first payout realistically lands **60–90 days** after the first sale. Seller requirements: tax interview (W-8BEN-E for non-US sellers, not needed for India-based sellers **[verified]**), bank account and a public listing review. All three hyperscalers let a single small company sell; none requires a minimum size **[unverified]**.

## 2. Business models

| Model | Fit | Verdict |
|---|---|---|
| **Open core + hosted control plane** | High. The OSS stays a single-writer CLI; teams want delete, rename, audit and approvals in a UI | **Primary** |
| Hosted SaaS (BYO-bucket) | High. Customer data stays in their bucket and is encrypted client-side, so trust cost is low; we host only metadata and wrapped keys | Same product as above |
| Support / setup contracts | Immediate cash, does not scale | **Bootstrap revenue** |
| GitHub Sponsors / Open Collective | Usually tens of dollars a month for a new project **[unverified]** | Turn on, but don't count on it |
| Docker-Sponsored OSS | Free: removes Docker Hub pull limits and adds a badge **[unverified]** | Apply (distribution, not revenue) |
| BSL/FSL license | Protects against cloud resellers, but costs adoption, and carry's moat is the hosted plane, not the binary | **Don't**. Keep Apache-2.0 |

What goes in the paid tier, ranked by willingness to pay:
1. Web UI for **delete/rename/rollback with an audit log** (who changed what, when): the visible gap in a CLI-only write path.
2. **Team RBAC + SSO (SAML/OIDC)**: the usual enterprise gate.
3. **KMS integration** (AWS KMS / GCP KMS / Azure Key Vault wrapping the repo key) and key rotation.
4. **Fleet dashboard**: which containers are on which version, failed polls, fail-open events.
5. **Managed curated tool store**: signed, pinned, CVE-scanned builds of gh, kubectl, jq and similar tools.
6. Compliance exports (SOC 2-style change reports) and multi-bucket replication / DR: Enterprise only.

## 3. Comparables (product and price only)

| Product | What it does vs carry | Price tiers |
|---|---|---|
| Doppler | Hosted secrets with a strong CLI/UX; secrets live on their side | Team about $21/user/mo **[secondary]**; free/dev tier disputed by aggregators |
| Infisical | Open-source secrets manager with a self-hostable core; bills per identity, including machines | Free (≈5 identities), Pro about $18/identity/mo, Enterprise custom **[secondary]**. Users complain machine-identity billing balloons |
| HashiCorp Vault / HCP Vault | Heavy, powerful, ops burden; BSL-licensed since 2023 | HCP priced per cluster-hour **[unverified]** |
| chezmoi | Dotfiles for humans, git-based, free | Free. Shows the "personal" use case won't pay |
| Spacelift | IaC orchestration: adjacent buyer (platform team), not a competitor | Per-seat/worker tiers **[unverified]** |
| Teleport | Access plane; open-core with paid SSO/RBAC: the model to copy | Enterprise, quote-based **[unverified]** |
| ConfigCat | Feature flags with config-change audit UI | Free tier, then paid tiers billed per config/environment **[unverified]** |
| Coder / Gitpod / devcontainers | Rebuild the whole dev environment; carry restores state into any existing container | Coder OSS + per-user Enterprise **[unverified]** |

carry's position: none of them combine **files + env + tools**, client-side encryption with **your own bucket**, versioning with rollback, and fail-open in one static binary. The pitch: "Doppler-style UX, but your bucket holds only ciphertext, and it carries tools and config files too, not just secrets."

## 4. Recommendation

### Pricing proposal
| Tier | Price | Includes |
|---|---|---|
| OSS | Free | CLI, runtime, SDKs, public tool store |
| Team | **$49/mo flat** for up to 10 users and 50 states, then $5/state | Web UI (delete/rename/rollback), audit log (90 days), fleet dashboard |
| Business | **$299/mo** | SSO, RBAC, KMS, curated signed tool store, 1-year audit, email support |
| Enterprise | from $12k/yr, private offer | Compliance exports, replication, SLA, marketplace billing |

Bill per **state** (container identity), not per user, but flat-capped, which avoids the Infisical "machine identities blew up my bill" complaint. Prices are a starting hypothesis to test with the first 10 buyers.

### First 30 days (fastest path to first revenue)
1. Pricing page with **Lemon Squeezy or Paddle** checkout (5% + 50¢ **[secondary]**; merchant of record, so they handle VAT/GST; check payout fees to an Indian bank). Sell **"carry Team early access" at $49/mo** before the full UI ships.
2. **Fixed-price "carry setup"** ($1,500: migrate one cluster, KMS wiring, runbook) through the same checkout or a Stripe invoice. Funnel via the owner's audience (books, workshops, LinkedIn) and a Show HN.
3. Turn on GitHub Sponsors; apply for Docker-Sponsored OSS; add free listings on Artifact Hub and the DigitalOcean 1-click marketplace.
4. Build the minimum paid surface: a hosted web UI for delete/rename/rollback + audit log, reading the customer's bucket with a scoped key.

### 31–90 days
5. SSO + RBAC + fleet dashboard, which unlocks Business.
6. **AWS Marketplace SaaS listing** (3%), from an entity that can sell globally (decide India-only vs a US LLC; the owner decides this, since it is legal/money). Then Azure and GCP SaaS listings (3% each) once there is one paying enterprise to point to.
7. Curated signed tool store as a Business feature.
8. Goal by day 90: 10 paying Team accounts + 1 setup contract ≈ $2k MRR-equivalent. That is a signal, not the ₹15L target; carry is a cluster-3 dev-tool bet.

### Payment rails
- **Lemon Squeezy or Paddle** (merchant of record): default for self-serve global sales from India.
- **Stripe** (India account): invoices for contracts. Stripe India's export-payment rules make it harder for subscriptions **[unverified]**.
- **GitHub Sponsors**: goodwill.
- **Marketplace billing**: enterprise only, after the entity decision.

## Sources
- [AWS Marketplace listing fees](https://docs.aws.amazon.com/marketplace/latest/userguide/listing-fees.html)
- [AWS Marketplace regions and countries](https://docs.aws.amazon.com/marketplace/latest/userguide/regions-and-countries.html)
- [AWS Marketplace India-based sellers, INR (Nov 2025)](https://aws.amazon.com/about-aws/whats-new/2025/11/aws-marketplace-india-based-sellers-transactions-inr/)
- [Clazar: Google Cloud Marketplace fees (2026)](https://clazar.io/blog/google-cloud-marketplace-fees)
- [The Register: GCP cuts marketplace fee to 3%](https://www.theregister.com/2021/09/27/google_cloud_marketplace_fees/)
- [ChannelE2E: marketplace fees AWS/Azure/Google](https://channele2e.com/news/cloud-marketplace-software-revenue-fees-aws-azure-google)
- [CostBench: Doppler vs Infisical](https://costbench.com/compare/doppler-vs-infisical/)
- [Rework: Paddle vs Lemon Squeezy 2026](https://resources.rework.com/tools/billing-revenue/paddle-vs-lemon-squeezy)
- [Dodo Payments: Paddle vs Lemon Squeezy](https://dodopayments.com/blogs/paddle-vs-lemon-squeezy/)
