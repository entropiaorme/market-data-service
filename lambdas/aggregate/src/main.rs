//! Aggregation function, invoked on an EventBridge schedule.
//!
//! Scans the observation table, folds observations into one aggregate entry
//! per (item, tier) (newest observation wins), validates the result against
//! the v1 snapshot schema, and publishes it: an immutable timestamped object
//! plus an atomically-replaced `latest` pointer. Publication is skipped when
//! the aggregate content is identical to what is already published, so the
//! snapshot history only grows when the data moved.

use aws_sdk_dynamodb::types::AttributeValue;
use lambda_runtime::{run, service_fn, Error, LambdaEvent};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const LATEST_KEY: &str = "v1/latest.json";

/// One parsed observation row from the table.
#[derive(Debug, Clone)]
struct Observation {
    item_name: String,
    tier: u8,
    observed_at: String,
    contributor_id: String,
    readings: contracts::Readings,
}

struct Handler {
    db: aws_sdk_dynamodb::Client,
    s3: aws_sdk_s3::Client,
    table_name: String,
    snapshot_bucket: String,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    lambda_runtime::tracing::init_default_subscriber();
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let handler = Handler {
        db: aws_sdk_dynamodb::Client::new(&config),
        s3: aws_sdk_s3::Client::new(&config),
        table_name: std::env::var("TABLE_NAME").expect("TABLE_NAME is set"),
        snapshot_bucket: std::env::var("SNAPSHOT_BUCKET").expect("SNAPSHOT_BUCKET is set"),
    };
    let handler = &handler;
    run(service_fn(move |event: LambdaEvent<Value>| async move {
        handler.handle(event).await
    }))
    .await
}

impl Handler {
    async fn handle(&self, _event: LambdaEvent<Value>) -> Result<Value, Error> {
        let observations = self.scan_observations().await?;
        let generated_at = OffsetDateTime::now_utc();
        let snapshot = fold_observations(&observations, &rfc3339(generated_at)?);
        tracing::info!(
            observations = observations.len(),
            items = snapshot.items.len(),
            contributors = snapshot.contributor_count,
            "folded observations"
        );

        let snapshot_value = serde_json::to_value(&snapshot)?;
        if let Err(errors) = contracts::validate_snapshot(&snapshot_value) {
            return Err(Error::from(format!(
                "refusing to publish a snapshot that fails its own schema: {errors:?}"
            )));
        }

        if let Some(published) = self.published_latest().await? {
            if same_content(&published, &snapshot) {
                tracing::info!("aggregate content unchanged; skipping publication");
                return Ok(
                    serde_json::json!({ "published": false, "items": snapshot.items.len() }),
                );
            }
        }

        let snapshot_key = format!("v1/snapshots/{}.json", compact_timestamp(generated_at)?);
        let body = serde_json::to_vec_pretty(&snapshot_value)?;
        self.put_json(
            &snapshot_key,
            body.clone(),
            "public, max-age=31536000, immutable",
        )
        .await?;
        self.put_json(LATEST_KEY, body, "public, max-age=300, must-revalidate")
            .await?;
        tracing::info!(key = %snapshot_key, "published snapshot");
        Ok(
            serde_json::json!({ "published": true, "key": snapshot_key, "items": snapshot.items.len() }),
        )
    }

    /// Read every observation row. A full scan is the deliberate choice at
    /// this table's size (thousands of rows); the aggregation runs once a
    /// day, and a scan of that volume costs fractions of a cent. Revisit
    /// with a GSI if the table ever grows past that honestly.
    async fn scan_observations(&self) -> Result<Vec<Observation>, Error> {
        let mut observations = Vec::new();
        let mut pages = self
            .db
            .scan()
            .table_name(&self.table_name)
            .filter_expression("begins_with(pk, :item)")
            .expression_attribute_values(":item", AttributeValue::S("item#".to_string()))
            .into_paginator()
            .items()
            .send();
        while let Some(row) = pages.next().await {
            let row = row?;
            match parse_observation(&row) {
                Ok(observation) => observations.push(observation),
                Err(reason) => {
                    // A malformed row is a bug, not a reason to publish
                    // nothing; log it loudly and aggregate the rest.
                    tracing::error!(reason, "skipping malformed observation row");
                }
            }
        }
        Ok(observations)
    }

    async fn published_latest(&self) -> Result<Option<contracts::Snapshot>, Error> {
        let response = self
            .s3
            .get_object()
            .bucket(&self.snapshot_bucket)
            .key(LATEST_KEY)
            .send()
            .await;
        let object = match response {
            Ok(object) => object,
            Err(err) if err.as_service_error().map(|e| e.is_no_such_key()) == Some(true) => {
                return Ok(None)
            }
            Err(err) => return Err(err.into()),
        };
        let bytes = object.body.collect().await?.into_bytes();
        Ok(serde_json::from_slice(&bytes).ok())
    }

    async fn put_json(&self, key: &str, body: Vec<u8>, cache_control: &str) -> Result<(), Error> {
        self.s3
            .put_object()
            .bucket(&self.snapshot_bucket)
            .key(key)
            .body(body.into())
            .content_type("application/json")
            .cache_control(cache_control)
            .send()
            .await?;
        Ok(())
    }
}

/// Fold observations into a snapshot: per (item, tier) the newest
/// observation wins; the contributor count is the number of distinct
/// contributors across the whole observation set. Staleness is the
/// client's to surface, from each entry's `observedAt`.
fn fold_observations(observations: &[Observation], generated_at: &str) -> contracts::Snapshot {
    let mut newest: HashMap<(String, u8), &Observation> = HashMap::new();
    let mut contributors: HashSet<&str> = HashSet::new();
    for observation in observations {
        contributors.insert(&observation.contributor_id);
        let key = (observation.item_name.to_lowercase(), observation.tier);
        let entry = newest.entry(key).or_insert(observation);
        // RFC 3339 UTC timestamps compare correctly as strings.
        if observation.observed_at > entry.observed_at {
            *entry = observation;
        }
    }
    let mut items: Vec<contracts::ItemAggregate> = newest
        .into_values()
        .map(|observation| contracts::ItemAggregate {
            item_name: observation.item_name.clone(),
            tier: observation.tier,
            observed_at: observation.observed_at.clone(),
            readings: observation.readings.clone(),
        })
        .collect();
    items.sort_by(|a, b| {
        (a.item_name.to_lowercase(), a.tier).cmp(&(b.item_name.to_lowercase(), b.tier))
    });
    contracts::Snapshot {
        schema_version: 1,
        generated_at: generated_at.to_string(),
        contributor_count: contributors.len(),
        items,
    }
}

/// Everything except `generatedAt`: two snapshots with the same items and
/// contributor count are the same publication.
fn same_content(a: &contracts::Snapshot, b: &contracts::Snapshot) -> bool {
    a.schema_version == b.schema_version
        && a.contributor_count == b.contributor_count
        && a.items == b.items
}

fn parse_observation(row: &HashMap<String, AttributeValue>) -> Result<Observation, String> {
    let string = |name: &str| -> Result<String, String> {
        match row.get(name) {
            Some(AttributeValue::S(value)) => Ok(value.clone()),
            other => Err(format!(
                "attribute {name} missing or not a string: {other:?}"
            )),
        }
    };
    let tier = match row.get("tier") {
        Some(AttributeValue::N(value)) => value
            .parse::<u8>()
            .map_err(|_| format!("tier is not a small integer: {value}"))?,
        other => return Err(format!("attribute tier missing or not a number: {other:?}")),
    };
    let readings: contracts::Readings = serde_json::from_str(&string("readingsJson")?)
        .map_err(|err| format!("readingsJson does not parse: {err}"))?;
    Ok(Observation {
        item_name: string("itemName")?,
        tier,
        observed_at: string("observedAt")?,
        contributor_id: string("contributorId")?,
        readings,
    })
}

fn rfc3339(instant: OffsetDateTime) -> Result<String, Error> {
    instant
        .format(&Rfc3339)
        .map_err(|err| Error::from(err.to_string()))
}

/// `20260712T103000Z`: filename-safe, lexicographically ordered.
fn compact_timestamp(instant: OffsetDateTime) -> Result<String, Error> {
    let format = time::macros::format_description!("[year][month][day]T[hour][minute][second]Z");
    instant
        .format(&format)
        .map_err(|err| Error::from(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(markup: Option<f64>) -> contracts::Reading {
        contracts::Reading {
            markup_pct: markup,
            sales_ped: 100.0,
        }
    }

    fn readings(markup: f64) -> contracts::Readings {
        contracts::Readings {
            day: reading(Some(markup)),
            week: reading(Some(markup)),
            month: reading(Some(markup)),
            year: reading(Some(markup)),
            decade: reading(None),
        }
    }

    fn observation(
        name: &str,
        tier: u8,
        observed_at: &str,
        contributor: &str,
        markup: f64,
    ) -> Observation {
        Observation {
            item_name: name.to_string(),
            tier,
            observed_at: observed_at.to_string(),
            contributor_id: contributor.to_string(),
            readings: readings(markup),
        }
    }

    const GENERATED_AT: &str = "2026-07-12T12:00:00Z";

    #[test]
    fn newest_observation_wins_per_item() {
        let snapshot = fold_observations(
            &[
                observation("Animal Muscle Oil", 0, "2026-07-01T00:00:00Z", "a", 101.0),
                observation("Animal Muscle Oil", 0, "2026-07-10T00:00:00Z", "a", 102.0),
                observation("Animal Muscle Oil", 0, "2026-07-05T00:00:00Z", "b", 103.0),
            ],
            GENERATED_AT,
        );
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].observed_at, "2026-07-10T00:00:00Z");
        assert_eq!(snapshot.items[0].readings.day.markup_pct, Some(102.0));
        assert_eq!(snapshot.contributor_count, 2);
    }

    #[test]
    fn item_names_fold_case_insensitively_but_keep_display_case() {
        let snapshot = fold_observations(
            &[
                observation("animal muscle oil", 0, "2026-07-01T00:00:00Z", "a", 101.0),
                observation("Animal Muscle Oil", 0, "2026-07-10T00:00:00Z", "a", 102.0),
            ],
            GENERATED_AT,
        );
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].item_name, "Animal Muscle Oil");
    }

    #[test]
    fn tiers_of_the_same_item_stay_separate() {
        let snapshot = fold_observations(
            &[
                observation("Improved Blade", 1, "2026-07-01T00:00:00Z", "a", 110.0),
                observation("Improved Blade", 2, "2026-07-01T00:00:00Z", "a", 120.0),
            ],
            GENERATED_AT,
        );
        assert_eq!(snapshot.items.len(), 2);
        assert_eq!(snapshot.items[0].tier, 1);
        assert_eq!(snapshot.items[1].tier, 2);
    }

    #[test]
    fn items_sort_by_name_then_tier() {
        let snapshot = fold_observations(
            &[
                observation("zinc", 0, "2026-07-01T00:00:00Z", "a", 101.0),
                observation("Animal Muscle Oil", 0, "2026-07-01T00:00:00Z", "a", 101.0),
            ],
            GENERATED_AT,
        );
        let names: Vec<&str> = snapshot
            .items
            .iter()
            .map(|i| i.item_name.as_str())
            .collect();
        assert_eq!(names, ["Animal Muscle Oil", "zinc"]);
    }

    #[test]
    fn an_empty_table_folds_to_an_empty_valid_snapshot() {
        let snapshot = fold_observations(&[], GENERATED_AT);
        assert_eq!(snapshot.items.len(), 0);
        assert_eq!(snapshot.contributor_count, 0);
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(contracts::validate_snapshot(&value), Ok(()));
    }

    #[test]
    fn folded_snapshots_validate_against_the_published_schema() {
        let snapshot = fold_observations(
            &[observation(
                "Animal Muscle Oil",
                0,
                "2026-07-01T00:00:00Z",
                "a",
                101.0,
            )],
            GENERATED_AT,
        );
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(contracts::validate_snapshot(&value), Ok(()));
    }

    #[test]
    fn same_content_ignores_generated_at() {
        let a = fold_observations(
            &[observation(
                "Animal Muscle Oil",
                0,
                "2026-07-01T00:00:00Z",
                "a",
                101.0,
            )],
            "2026-07-12T12:00:00Z",
        );
        let b = fold_observations(
            &[observation(
                "Animal Muscle Oil",
                0,
                "2026-07-01T00:00:00Z",
                "a",
                101.0,
            )],
            "2026-07-13T12:00:00Z",
        );
        assert!(same_content(&a, &b));
    }

    #[test]
    fn same_content_detects_moved_data() {
        let a = fold_observations(
            &[observation(
                "Animal Muscle Oil",
                0,
                "2026-07-01T00:00:00Z",
                "a",
                101.0,
            )],
            GENERATED_AT,
        );
        let b = fold_observations(
            &[observation(
                "Animal Muscle Oil",
                0,
                "2026-07-02T00:00:00Z",
                "a",
                102.0,
            )],
            GENERATED_AT,
        );
        assert!(!same_content(&a, &b));
    }

    #[test]
    fn compact_timestamps_are_filename_safe() {
        let instant = OffsetDateTime::parse("2026-07-12T10:30:00Z", &Rfc3339).unwrap();
        assert_eq!(compact_timestamp(instant).unwrap(), "20260712T103000Z");
    }
}
