//! Ingest function: POST /v1/submissions.
//!
//! Authenticates the contributor token, validates the body against the v1
//! submission schema, and writes one observation item per (item, tier) to the
//! observation table. Writes are idempotent: the key is derived from the
//! item, the observation timestamp, and the contributor, so a replayed
//! submission overwrites itself instead of duplicating.

use aws_sdk_dynamodb::types::{AttributeValue, PutRequest, WriteRequest};
use lambda_http::{run, service_fn, Body, Error, Request, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Observations expire out of the table after two years; the published
/// snapshot carries staleness explicitly long before that.
const OBSERVATION_TTL: Duration = Duration::from_secs(2 * 365 * 24 * 60 * 60);

/// Reject observation timestamps more than an hour ahead of the server
/// clock; honest clients report the time the reading was taken.
const MAX_FUTURE_SKEW: Duration = Duration::from_secs(60 * 60);

const BATCH_WRITE_LIMIT: usize = 25;
const BATCH_RETRY_LIMIT: u32 = 3;

struct Handler {
    db: aws_sdk_dynamodb::Client,
    table_name: String,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    lambda_http::tracing::init_default_subscriber();
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let handler = Handler {
        db: aws_sdk_dynamodb::Client::new(&config),
        table_name: std::env::var("TABLE_NAME").expect("TABLE_NAME is set"),
    };
    let handler = &handler;
    run(service_fn(move |request| async move {
        handler.handle(request).await
    }))
    .await
}

impl Handler {
    async fn handle(&self, request: Request) -> Result<Response<Body>, Error> {
        let Some((contributor_id, secret)) = bearer_token(&request) else {
            return json_response(401, json!({ "error": "missing or malformed bearer token" }));
        };
        if !self.authenticate(&contributor_id, &secret).await? {
            tracing::warn!(contributor = %contributor_id, "rejected contributor token");
            return json_response(401, json!({ "error": "unknown or inactive contributor" }));
        }

        let body: Value = match serde_json::from_slice(request.body().as_ref()) {
            Ok(body) => body,
            Err(err) => {
                return json_response(400, json!({ "error": format!("body is not JSON: {err}") }))
            }
        };
        if let Err(errors) = contracts::validate_submission(&body) {
            return json_response(
                400,
                json!({ "error": "schema validation failed", "details": errors }),
            );
        }
        let submission: contracts::Submission =
            serde_json::from_value(body).map_err(|err| Error::from(err.to_string()))?;

        let observed_at = match normalised_observed_at(&submission.observed_at) {
            Ok(ts) => ts,
            Err(reason) => return json_response(400, json!({ "error": reason })),
        };

        let stored = self
            .store_observations(&submission, &observed_at, &contributor_id)
            .await?;
        tracing::info!(
            contributor = %contributor_id,
            observed_at = %observed_at,
            items = stored,
            "stored submission"
        );
        json_response(200, json!({ "stored": stored, "observedAt": observed_at }))
    }

    /// Look the contributor up and compare the presented secret's digest
    /// against the stored one in constant time.
    async fn authenticate(&self, contributor_id: &str, secret: &str) -> Result<bool, Error> {
        let record = self
            .db
            .get_item()
            .table_name(&self.table_name)
            .key(
                "pk",
                AttributeValue::S(contracts::contributor_partition_key(contributor_id)),
            )
            .key("sk", AttributeValue::S("meta".to_string()))
            .send()
            .await?;
        let Some(item) = record.item else {
            return Ok(false);
        };
        let active = matches!(item.get("active"), Some(AttributeValue::Bool(true)));
        let Some(AttributeValue::S(stored_hash)) = item.get("tokenHash") else {
            return Ok(false);
        };
        let presented_hash = hex::encode(Sha256::digest(secret.as_bytes()));
        Ok(active && constant_time_eq(stored_hash.as_bytes(), presented_hash.as_bytes()))
    }

    async fn store_observations(
        &self,
        submission: &contracts::Submission,
        observed_at: &str,
        contributor_id: &str,
    ) -> Result<usize, Error> {
        let submitted_at = OffsetDateTime::now_utc();
        let expires_at = (submitted_at + OBSERVATION_TTL).unix_timestamp();
        let submitted_at = submitted_at
            .format(&Rfc3339)
            .map_err(|err| Error::from(err.to_string()))?;

        let writes: Vec<WriteRequest> = submission
            .items
            .iter()
            .map(|item| {
                let readings_json = serde_json::to_string(&item.readings)
                    .map_err(|err| Error::from(err.to_string()))?;
                let put = PutRequest::builder()
                    .item(
                        "pk",
                        AttributeValue::S(contracts::item_partition_key(
                            &item.item_name,
                            item.tier,
                        )),
                    )
                    .item(
                        "sk",
                        AttributeValue::S(format!("obs#{observed_at}#{contributor_id}")),
                    )
                    .item("itemName", AttributeValue::S(item.item_name.clone()))
                    .item("tier", AttributeValue::N(item.tier.to_string()))
                    .item("observedAt", AttributeValue::S(observed_at.to_string()))
                    .item(
                        "contributorId",
                        AttributeValue::S(contributor_id.to_string()),
                    )
                    .item("submittedAt", AttributeValue::S(submitted_at.clone()))
                    .item("readingsJson", AttributeValue::S(readings_json))
                    .item("expiresAt", AttributeValue::N(expires_at.to_string()))
                    .build()
                    .map_err(|err| Error::from(err.to_string()))?;
                Ok(WriteRequest::builder().put_request(put).build())
            })
            .collect::<Result<_, Error>>()?;

        for chunk in writes.chunks(BATCH_WRITE_LIMIT) {
            self.write_batch(chunk.to_vec()).await?;
        }
        Ok(writes.len())
    }

    /// One BatchWriteItem call, re-driving unprocessed items with a short
    /// backoff; DynamoDB returns partial success under throttling.
    async fn write_batch(&self, mut pending: Vec<WriteRequest>) -> Result<(), Error> {
        for attempt in 0..=BATCH_RETRY_LIMIT {
            let outcome = self
                .db
                .batch_write_item()
                .request_items(&self.table_name, pending)
                .send()
                .await?;
            pending = outcome
                .unprocessed_items
                .unwrap_or_default()
                .remove(&self.table_name)
                .unwrap_or_default();
            if pending.is_empty() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt + 1))).await;
        }
        Err(Error::from(format!(
            "{} observation writes still unprocessed after {BATCH_RETRY_LIMIT} retries",
            pending.len()
        )))
    }
}

/// Extract `(contributor_id, secret)` from `Authorization: Bearer <id>.<secret>`.
fn bearer_token(request: &Request) -> Option<(String, String)> {
    let header = request.headers().get("authorization")?.to_str().ok()?;
    let token = header.strip_prefix("Bearer ")?;
    let (id, secret) = token.split_once('.')?;
    let id_is_wellformed = !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if id_is_wellformed && !secret.is_empty() {
        Some((id.to_string(), secret.to_string()))
    } else {
        None
    }
}

/// Parse and normalise the submission's `observedAt` to UTC RFC 3339 so key
/// ordering and idempotency never depend on the client's offset formatting.
fn normalised_observed_at(raw: &str) -> Result<String, String> {
    let parsed = OffsetDateTime::parse(raw, &Rfc3339)
        .map_err(|_| format!("observedAt is not an RFC 3339 timestamp: {raw}"))?;
    if parsed - OffsetDateTime::now_utc() > MAX_FUTURE_SKEW {
        return Err("observedAt is in the future".to_string());
    }
    parsed
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|err| err.to_string())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn json_response(status: u16, body: Value) -> Result<Response<Body>, Error> {
    Ok(Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_auth(header: Option<&str>) -> Request {
        let mut request = Request::default();
        if let Some(value) = header {
            request
                .headers_mut()
                .insert("authorization", value.parse().unwrap());
        }
        request
    }

    #[test]
    fn bearer_token_parses_a_wellformed_token() {
        let request = request_with_auth(Some("Bearer contributor-1.s3cr3t-value"));
        assert_eq!(
            bearer_token(&request),
            Some(("contributor-1".to_string(), "s3cr3t-value".to_string()))
        );
    }

    #[test]
    fn bearer_token_keeps_dots_in_the_secret() {
        let request = request_with_auth(Some("Bearer id-1.part.two"));
        assert_eq!(
            bearer_token(&request),
            Some(("id-1".to_string(), "part.two".to_string()))
        );
    }

    #[test]
    fn bearer_token_rejects_malformed_headers() {
        for header in [
            None,
            Some("Basic abc"),
            Some("Bearer no-separator"),
            Some("Bearer .secret-only"),
            Some("Bearer id."),
            Some("Bearer UPPER.secret"),
        ] {
            assert_eq!(bearer_token(&request_with_auth(header)), None, "{header:?}");
        }
    }

    #[test]
    fn observed_at_normalises_offsets_to_utc() {
        assert_eq!(
            normalised_observed_at("2026-07-12T12:30:00+02:00").unwrap(),
            "2026-07-12T10:30:00Z"
        );
    }

    #[test]
    fn observed_at_rejects_future_timestamps() {
        let err = normalised_observed_at("2030-01-01T00:00:00Z").unwrap_err();
        assert!(err.contains("future"));
    }

    #[test]
    fn observed_at_rejects_non_timestamps() {
        assert!(normalised_observed_at("last tuesday").is_err());
    }

    #[test]
    fn constant_time_eq_compares_correctly() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
