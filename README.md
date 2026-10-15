# EntropiaOrme market-data service

The central market-data backend for [EntropiaOrme](https://github.com/entropiaorme/entropiaorme):
a small AWS serverless pipeline that receives per-item markup observations from
authenticated clients, aggregates them on a schedule, and publishes versioned
snapshot files that every installation of the app can consume.

**Status: retired.** The service ran from July to October 2026 and has been
torn down: every AWS resource it created is destroyed, and the app now keeps
market observations in its own local store. The repository stays as a
reference for the design; the Terraform here is configured so a fresh
deployment can also be destroyed cleanly. The design was contributor-agnostic
from the first commit: the same authenticated ingest path served one
contributor or many, and aggregation and outlier handling were designed for N
contributors even while N was small.

## The pipeline

```
client paste (in-app)                       every installation
      |                                            ^
      v                                            |
  POST /submissions  ->  observation store  ->  scheduled aggregation
  (API Gateway +         (DynamoDB)             (Lambda, EventBridge)
   Lambda: auth,                                     |
   schema validation,                                v
   idempotent write)                    versioned snapshot (S3 + CloudFront,
                                        atomic publish via a latest pointer)
```

- **Ingest** accepts schema-versioned submissions (see [`schemas/`](schemas/))
  from authenticated contributors, validates them, and stores observations with
  per-contributor provenance.
- **Aggregation** runs on a schedule, folds observations into per-item aggregate
  readings, and writes an immutable, versioned snapshot object.
- **Distribution** publishes snapshots behind CloudFront; clients fetch with
  ETag-based caching and surface data staleness in the app rather than hiding it.

All market data handled here is public information from the game's own auction
house; submissions carry no personal data. In the app, centrally-aggregated
markup remains an informational layer: estimates never enter recorded
profit-and-loss figures.

## Repository layout

| Path | Contents |
| --- | --- |
| `terraform/` | All infrastructure as code; no console-created resources. The runbook is [`terraform/README.md`](terraform/README.md); `bootstrap/` stands up the Terraform state backend and the cost guardrails first, `main/` is the service itself. |
| `schemas/` | Versioned JSON Schemas for submissions and published snapshots, with an explicit compatibility policy (documented alongside the schemas). Published copies are served under `/schemas/` on the service domain, matching each schema's `$id`. |
| `lambdas/` | The Rust workspace behind the pipeline: `contracts` (shared types; validates against the embedded schemas), `ingest` (auth, validation, idempotent writes), `aggregate` (fold, validate, atomic publish). Built with cargo-lambda for arm64. |

## Operating principles

- **Infrastructure as code, entirely.** Every resource is Terraform-managed;
  the bootstrap module and runbook document the order of operations from an
  empty account.
- **Least-privilege IAM.** Each function gets its own role scoped to exactly
  the resources it touches.
- **Cost honesty.** This is a deliberately small, low-traffic service and its guardrails say so: budget alarms
  are part of the first apply, and the cost model is documented before the
  first production request.
- **Versioned contracts.** Submission and snapshot schemas are versioned with
  an explicit compatibility policy; breaking changes mint a new version rather
  than mutating an old one.

## Licence

[MIT](LICENSE)
