# AWS Marketplace - container product usage instructions (paste into "Usage instructions")

Delivery option 1: Container image (ECS / EKS / Docker)
1. Authenticate: `aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin 709825985650.dkr.ecr.us-east-1.amazonaws.com`
2. Use the image in your Dockerfile:
   `COPY --from=709825985650.dkr.ecr.us-east-1.amazonaws.com/deemwar/carry:0.1.1 /carry /usr/local/bin/carry`
   `ENTRYPOINT ["carry","run","--"]`
3. Set env: `CARRY_REPO=s3://<bucket>/carry`, `CARRY_STATE=<name>`, `CARRY_S3_ENDPOINT=https://s3.<region>.amazonaws.com`,
   `CARRY_KEY_FILE=/run/secrets/carry-key`. On EKS prefer IRSA / Pod Identity instead of static keys
   (the role needs s3:GetObject, s3:PutObject, s3:ListBucket on the prefix).

Delivery option 2: Helm chart
`helm install carry oci://709825985650.dkr.ecr.us-east-1.amazonaws.com/deemwar/carry-chart --version 0.1.1 --set repo=s3://<bucket>/carry`

(Repository paths are assigned by AWS after "Add repository" in the Seller Portal - replace placeholders.)
Support: https://github.com/muthuishere/carry/issues  -  EULA: Apache-2.0 (use custom EULA link to LICENSE).
