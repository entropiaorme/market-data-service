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

## 2. Main stack (arrives with the service resources)

The main stack (ingest, storage, aggregation, distribution) backs onto the
bootstrap bucket with Terraform's native S3 lockfile locking:

```hcl
backend "s3" {
  bucket       = "<state_bucket from bootstrap>"
  key          = "market-data/main.tfstate"
  region       = "<aws_region>"
  use_lockfile = true
}
```

Applies run from the runbook (documented plan/apply, reviewed diffs) until a
CI pipeline earns its keep; the state backend and locking are CI-ready from
day one.
