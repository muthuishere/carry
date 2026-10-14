# carry on every marketplace

Paid-listing strategy, fees and payouts: see [MONETIZATION.md](./MONETIZATION.md) (maintained separately).

**Verification note.** Requirements below come from each vendor's public seller docs (links cited) as known
up to mid-2026; they were NOT re-browsed live in this pass. Rows marked **(unverified)** are the weakest - re-check
the linked page before submitting. Fees change; treat numbers as indicative.

Shared copy for every listing: [listing/listing.md](listing/listing.md). Logo: [logo.svg](logo.svg). EULA: Apache-2.0.

## Status

| Channel | State | Notes |
|---|---|---|
| Homebrew tap | **Listed now** | `brew install muthuishere/tap/carry` - tested on macOS arm64 (source build, ~1 min), Linux uses release binaries |
| Docker Hub / GHCR | Live already | Sponsored OSS badge = owner applies |
| Artifact Hub | Ready for owner | needs Helm repo published + "Add repository" |
| DigitalOcean 1-Click K8s | Ready for owner | PR to marketplace-kubernetes + vendor portal |
| AWS / GCP / Azure / Oracle / Red Hat | Ready skeletons, blocked on seller account | see table |
| Scoop | Blocked | no Windows binary in v0.1.1 |
| apt (.deb) | Not done | needs `cargo deb` in release workflow |
| cargo binstall | Not done | needs `[package.metadata.binstall]` in Cargo.toml (src/ owner) + crates.io publish |

## Per marketplace

| Marketplace | Cost to list | Seller requirements | Artifact format | Paid options (can carry price later?) | Review time | Ready here | Owner actions |
|---|---|---|---|---|---|---|---|
| **AWS Marketplace** - container product | Free to list; free products pay no listing fee. Paid: AWS fee 3% (most SaaS/containers since 2024) | AWS account; seller registration in AWS Marketplace Management Portal; for paid: tax interview (W-8BEN-E for non-US entity), bank account (US/eligible), KYC. Free listings need only public seller profile. | Images (and Helm chart) pushed to AWS-owned ECR repos created per product; EKS add-on is an extra option (needs Helm chart passing add-on requirements, no `lookup`/hooks restrictions) | Hourly/monthly per-task/pod, contract, BYOL, private offers - yes | ~2-4 weeks incl. image scan | [aws/usage-instructions.md](aws/usage-instructions.md), listing copy, logo | Register: https://aws.amazon.com/marketplace/management/ -> Products -> Server -> Container. Docs: https://docs.aws.amazon.com/marketplace/latest/userguide/container-based-products.html , EKS add-on: https://docs.aws.amazon.com/marketplace/latest/userguide/container-product-policies.html . Choose **paid-capable** "Container" not "free" if a price may come later (free->paid needs new pricing review). |
| **Google Cloud Marketplace** - Kubernetes app | Free to list; GCP takes 3% on paid | Join **Partner Advantage** (free Build membership), Producer Portal access, GCP project, Cloud Billing; for paid: payments profile, tax, bank; legal entity | Deployer image (`deployer_helm`) + app images in your Artifact Registry, `schema.yaml`, `application.yaml` CR; passes `mpdev verify` | Usage-based (per vCPU/pod), subscription, private offers - yes (needs usage reporting agent for metered) | ~2-6 weeks **(unverified)** | [gcp/schema.yaml](gcp/schema.yaml), [gcp/Dockerfile.deployer](gcp/Dockerfile.deployer) | https://cloud.google.com/marketplace/docs/partners , https://github.com/GoogleCloudPlatform/marketplace-k8s-app-tools . Join Partner Advantage: https://partners.cloud.google.com |
| **Azure Marketplace** - Kubernetes application offer | Free; 3% fee on paid | Partner Center account + Microsoft AI Cloud Partner Program enrollment, verified legal entity (business verification, can take days), tax profile + payout account for paid | CNAB bundle built with CPA tool (`container-package-app`), pushed to your ACR; ARM `mainTemplate.json` + `createUIDefinition.json`; deploys as AKS cluster extension | Free, BYOL, per-core/cluster/node usage, private offers - yes | ~1-3 weeks certification | [azure/](azure/) manifest, ARM, UI def skeletons | https://partner.microsoft.com/dashboard -> Marketplace offers -> Azure Container -> Kubernetes application. Docs: https://learn.microsoft.com/partner-center/marketplace-offers/azure-container-technical-assets-kubernetes |
| **DigitalOcean** 1-Click K8s | Free | DO account, Vendor Portal access (apply), support contact | PR adding `stacks/carry/{deploy,upgrade,uninstall}.sh,values.yml` to github.com/digitalocean/marketplace-kubernetes | No direct billing for K8s 1-clicks (free only) - SaaS Add-ons is the paid path | ~1-2 weeks | [digitalocean/carry/](digitalocean/carry/) | Publish Helm repo first; fork repo, PR; https://marketplace.digitalocean.com/vendors |
| **Akamai / Linode** Marketplace | Free | Partner application (Akamai Qualified Compute / marketplace partner form) | StackScript / Ansible playbook in github.com/akamai-compute-marketplace; Kubernetes apps (LKE) currently limited **(unverified)** | No paid billing via Linode marketplace **(unverified)** | weeks | listing copy | https://www.linode.com/marketplace/ -> "Submit an app". Low value for an image-embedded tool; defer |
| **Oracle Cloud Marketplace** | Free | Oracle PartnerNetwork (OPN) membership (free tier), Partner Portal publisher account | Container image (OCIR) / Helm, or Stack (Terraform) | BYOL and paid (usage) | ~2-4 weeks **(unverified)** | listing copy, logo | https://docs.oracle.com/iaas/Content/Marketplace/publisher.htm |
| **Red Hat** OpenShift certification | Free (Red Hat Partner Connect) | Partner Connect account, company info; containers must pass preflight (UBI base or scratch allowed for certified container?) **(unverified for scratch)** | Certified container image (`preflight check container`), Helm chart cert via `chart-verifier` PR to openshift-helm-charts, or Operator bundle for OperatorHub | Red Hat Marketplace (paid) was wound down 2024; paid via AWS/Azure OpenShift listings | 1-3 weeks | listing copy | https://connect.redhat.com . Helm cert: https://github.com/openshift-helm-charts/charts . Community OperatorHub needs an operator (carry has none) - skip |
| **Artifact Hub** (Helm) | Free | GitHub login, repo ownership claim | Helm repo (HTTP index.yaml or OCI) + `artifacthub-repo.yml` | n/a | Minutes (indexed every ~30 min) | [artifacthub/](artifacthub/) repo metadata + Chart annotations | Publish chart (gh-pages via chart-releaser or OCI on ghcr), https://artifacthub.io/control-panel -> Add repository, paste repositoryID into artifacthub-repo.yml |
| **Docker Hub** Sponsored OSS / Verified | OSS program free; Verified Publisher is paid partnership | OSS: public repo, OSI license, active development, no commercial pathway **(note: conflicts with future paid plans)** | Existing image | Verified Publisher = commercial | ~2-4 weeks | n/a | https://www.docker.com/community/open-source/application/ |
| **Civo** Marketplace | Free | GitHub PR | Add app dir to github.com/civo/kubernetes-marketplace (app.yaml, install.sh, logo) | Free only | days | reuse DO scripts | Fork + PR |
| **Vultr** Marketplace | Free; Vultr shares revenue on paid apps | Vendor signup | Snapshot/VM image or K8s app **(unverified)** | Paid apps possible | weeks | listing copy | https://www.vultr.com/marketplace/become-a-verified-vendor/ |
| **Alibaba Cloud** Marketplace | Free | Business license; typically mainland/Intl entity verification | Container image / Helm in ACR | Paid yes | long | listing copy | Low priority - entity verification heavy **(unverified)** |
| **Huawei Cloud KooGallery** | Free | Enterprise real-name verification | Container/Helm | Paid yes | long | listing copy | Low priority **(unverified)** |

## Install channels

- **Homebrew**: done - https://github.com/muthuishere/homebrew-tap/blob/main/Formula/carry.rb . Ship macOS binaries in the next release to avoid the Rust build (then switch `on_macos` to bottles/binary URLs).
- **Scoop**: no Windows build in v0.1.1. Add `x86_64-pc-windows-msvc` target to release, then a manifest goes in a bucket repo.
- **apt**: add `cargo deb` to the release workflow and attach `.deb`; optional apt repo via GitHub Pages.
- **cargo binstall**: add to Cargo.toml:
  ```toml
  [package.metadata.binstall]
  pkg-url = "{ repo }/releases/download/v{ version }/carry-{ target-family }-{ target-arch }"
  pkg-fmt = "bin"
  ```
  (Asset names use `amd64/arm64`; needs target-arch overrides or renamed assets. Also requires `cargo publish` to crates.io.)

## Recommended order

1. Artifact Hub (needs only a published chart) 2. DigitalOcean + Civo PRs 3. AWS container product as **paid-capable** (biggest buyer pool, private offers) 4. Azure K8s app 5. GCP (deployer work heaviest) 6. Red Hat Helm cert.
