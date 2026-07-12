# Infrastructure runbook

Every resource in this service is Terraform-managed; nothing is created in
the console. From an empty AWS account, the order of operations is:

## 0. One-time account prerequisites (console, by design)

The only console actions in the service's life are the account-level ones
Terraform cannot perform for itself:

1. Root user: enable MFA; never use root again.
2. Create the administrative identity Terraform runs as (an IAM Identity
   Center user, or an IAM user with an access key for the CLI) and configure
   the local AWS CLI profile for it.

## 1. Bootstrap stack (`bootstrap/`, local state)

Stands up the Terraform state bucket and the cost guardrails. It keeps local
state on the maintainer machine deliberately: the stack that creates the
state backend cannot back onto it.

```sh
cd terraform/bootstrap
terraform init
terraform apply \
  -var state_bucket_name=<globally-unique-name> \
  -var alert_email=<maintainer-email>
```

Budget alerts fire at 50% and 90% of actual spend and at 100% forecast
against a small monthly ceiling (default 5 USD), sized to what a
low-traffic service should actually cost.

## 2. Main stack (`main/`, S3-backed state)

The main stack (ingest, storage, aggregation, distribution, observability)
backs onto the bootstrap bucket with Terraform's native S3 lockfile locking
(the backend block in `main/versions.tf` names the bucket, the
`market-data/main.tfstate` key, and `use_lockfile = true`).

Applies run from this runbook (documented plan/apply, reviewed diffs) until
a CI pipeline earns its keep; the state backend and locking are CI-ready
from day one.

### 2.1 Build the functions

The Lambda binaries must exist before Terraform can zip them. From
`lambdas/`, with [cargo-lambda](https://www.cargo-lambda.info/) and zig on
the path:

```sh
cargo test --workspace
cargo lambda build --release --arm64
```

### 2.2 Plan and apply

```sh
cd terraform/main
terraform init
terraform plan -var alert_email=<maintainer-email> -out=main.tfplan
terraform apply main.tfplan
```

The first apply creates the whole pipeline with the distribution serving on
its `*.cloudfront.net` domain, and outputs two DNS records.

### 2.3 DNS and the custom domain (two-step by necessity)

CloudFront only accepts an alias once its certificate is ISSUED, and the
certificate only issues once its validation record exists in DNS. So:

1. Add both records from the outputs at the domain's DNS host: the ACM
   validation record (`acm_validation_records`) and the service CNAME
   (`domain_dns_record`, pointing at the distribution).
2. Wait for the certificate to read ISSUED
   (`aws acm describe-certificate --region us-east-1 ...`), then re-apply
   with the domain attached:

```sh
terraform apply -var alert_email=<maintainer-email> -var domain_ready=true
```

### 2.4 Mint a contributor

Contributor credentials are data, not infrastructure: the table row holds
only a SHA-256 digest of the secret, so the secret itself never touches
Terraform state. To mint one:

```sh
CONTRIBUTOR_ID=<id: lowercase letters, digits, hyphens>
SECRET=$(openssl rand -hex 32)
HASH=$(printf %s "$SECRET" | sha256sum | cut -d' ' -f1)
aws dynamodb put-item \
  --table-name entropiaorme-market-data \
  --item "{
    \"pk\": {\"S\": \"contributor#$CONTRIBUTOR_ID\"},
    \"sk\": {\"S\": \"meta\"},
    \"tokenHash\": {\"S\": \"$HASH\"},
    \"active\": {\"BOOL\": true}
  }"
echo "token: $CONTRIBUTOR_ID.$SECRET"
```

The client authenticates with `Authorization: Bearer <id>.<secret>`. To
revoke, flip `active` to false (or delete the row).

### 2.5 Verify the pipeline

```sh
# Ingest (through the distribution; the direct api_endpoint also works):
curl -X POST https://<distribution_domain>/v1/submissions \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d @submission.json

# Aggregate immediately instead of waiting for the schedule:
aws lambda invoke --function-name entropiaorme-market-data-aggregate /dev/stdout

# Fetch what clients will fetch:
curl https://<distribution_domain>/v1/latest.json
```

## Cost model

Everything scales to zero. At the expected cadence (one contributor batch
per week, one aggregation per day, modest snapshot fan-out) every line item
sits inside the always-free tier: Lambda (1M requests + 400k GB-s/month),
DynamoDB on-demand (25 GB storage; single-digit writes/week), EventBridge
(schedules are free), SNS/CloudWatch (10 alarms and 5 GB logs free),
CloudFront (1 TB egress + 10M requests/month). The only structural cost is
S3 storage growth from immutable snapshots (pennies per year at kilobyte
sizes; the aggregation skips publishing when nothing changed, so history
grows only when the data moves). The bootstrap stack's budget alarms at a
5 USD/month ceiling are the backstop for all of it.
