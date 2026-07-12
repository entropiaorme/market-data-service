//! Shared contract types and JSON Schema validation for the market-data
//! service. The schemas under `schemas/` are the source of truth; this crate
//! embeds them so every function validates against exactly what is published.

use jsonschema::Validator;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

pub const SUBMISSION_SCHEMA_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schemas/submission.v1.schema.json"
));

pub const SNAPSHOT_SCHEMA_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schemas/snapshot.v1.schema.json"
));

static SUBMISSION_VALIDATOR: LazyLock<Validator> =
    LazyLock::new(|| compile(SUBMISSION_SCHEMA_JSON));
static SNAPSHOT_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| compile(SNAPSHOT_SCHEMA_JSON));

fn compile(schema_json: &str) -> Validator {
    let schema: Value = serde_json::from_str(schema_json).expect("embedded schema is valid JSON");
    jsonschema::options()
        .should_validate_formats(true)
        .build(&schema)
        .expect("embedded schema compiles")
}

/// How many validation errors to report back before cutting off; enough to
/// fix a malformed submission without echoing hundreds of lines.
const MAX_REPORTED_ERRORS: usize = 8;

fn validate(validator: &Validator, instance: &Value) -> Result<(), Vec<String>> {
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .take(MAX_REPORTED_ERRORS)
        .map(|e| format!("{}: {}", e.instance_path, e))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate a submission body against the embedded v1 submission schema.
pub fn validate_submission(instance: &Value) -> Result<(), Vec<String>> {
    validate(&SUBMISSION_VALIDATOR, instance)
}

/// Validate a snapshot document against the embedded v1 snapshot schema.
pub fn validate_snapshot(instance: &Value) -> Result<(), Vec<String>> {
    validate(&SNAPSHOT_VALIDATOR, instance)
}

/// One markup/volume reading for a single time window. `markup_pct` is null
/// where the game reported N/A (no sales in that window).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reading {
    pub markup_pct: Option<f64>,
    pub sales_ped: f64,
}

/// The five time windows the game's market ledger reports per item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Readings {
    pub day: Reading,
    pub week: Reading,
    pub month: Reading,
    pub year: Reading,
    pub decade: Reading,
}

/// One item's readings inside a client submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ItemObservation {
    pub item_name: String,
    pub tier: u8,
    pub readings: Readings,
}

/// A client submission: one batch of observations taken at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Submission {
    pub schema_version: u32,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    pub items: Vec<ItemObservation>,
}

/// One item's aggregated entry in a published snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ItemAggregate {
    pub item_name: String,
    pub tier: u8,
    pub observed_at: String,
    pub readings: Readings,
}

/// A published aggregate snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub generated_at: String,
    pub contributor_count: usize,
    pub items: Vec<ItemAggregate>,
}

/// The DynamoDB partition key for an item's observations. Case-folded so the
/// same item never splits across partitions on caseing differences.
pub fn item_partition_key(item_name: &str, tier: u8) -> String {
    format!("item#{}#t{tier}", item_name.to_lowercase())
}

/// The DynamoDB partition key for a contributor record.
pub fn contributor_partition_key(contributor_id: &str) -> String {
    format!("contributor#{contributor_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_submission() -> Value {
        json!({
            "schemaVersion": 1,
            "observedAt": "2026-07-12T10:00:00Z",
            "clientVersion": "0.3.0",
            "items": [{
                "itemName": "Animal Muscle Oil",
                "tier": 0,
                "readings": {
                    "day":    { "markupPct": 101.5, "salesPed": 1200.0 },
                    "week":   { "markupPct": 101.9, "salesPed": 9100.0 },
                    "month":  { "markupPct": 102.1, "salesPed": 40000.0 },
                    "year":   { "markupPct": 103.0, "salesPed": 480000.0 },
                    "decade": { "markupPct": 104.2, "salesPed": 6400000.0 }
                }
            }]
        })
    }

    #[test]
    fn accepts_a_valid_submission() {
        assert_eq!(validate_submission(&valid_submission()), Ok(()));
    }

    #[test]
    fn accepts_null_markup_for_no_sales_windows() {
        let mut sub = valid_submission();
        sub["items"][0]["readings"]["day"] = json!({ "markupPct": null, "salesPed": 0.0 });
        assert_eq!(validate_submission(&sub), Ok(()));
    }

    #[test]
    fn rejects_a_missing_window() {
        let mut sub = valid_submission();
        sub["items"][0]["readings"]
            .as_object_mut()
            .unwrap()
            .remove("decade");
        assert!(validate_submission(&sub).is_err());
    }

    #[test]
    fn rejects_negative_sales_volume() {
        let mut sub = valid_submission();
        sub["items"][0]["readings"]["day"]["salesPed"] = json!(-1.0);
        assert!(validate_submission(&sub).is_err());
    }

    #[test]
    fn rejects_an_empty_item_batch() {
        let mut sub = valid_submission();
        sub["items"] = json!([]);
        assert!(validate_submission(&sub).is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let mut sub = valid_submission();
        sub["surprise"] = json!(true);
        assert!(validate_submission(&sub).is_err());
    }

    #[test]
    fn rejects_a_non_datetime_observed_at() {
        let mut sub = valid_submission();
        sub["observedAt"] = json!("last tuesday");
        assert!(validate_submission(&sub).is_err());
    }

    #[test]
    fn submission_round_trips_through_the_typed_form() {
        let value = valid_submission();
        let typed: Submission = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), value);
    }

    #[test]
    fn a_generated_snapshot_validates_against_the_snapshot_schema() {
        let typed: Submission = serde_json::from_value(valid_submission()).unwrap();
        let snapshot = Snapshot {
            schema_version: 1,
            generated_at: "2026-07-12T12:00:00Z".to_string(),
            contributor_count: 1,
            items: typed
                .items
                .into_iter()
                .map(|item| ItemAggregate {
                    item_name: item.item_name,
                    tier: item.tier,
                    observed_at: typed.observed_at.clone(),
                    readings: item.readings,
                })
                .collect(),
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(validate_snapshot(&value), Ok(()));
    }

    #[test]
    fn partition_keys_case_fold_item_names() {
        assert_eq!(
            item_partition_key("Animal Muscle Oil", 0),
            "item#animal muscle oil#t0"
        );
        assert_eq!(
            item_partition_key("ANIMAL muscle OIL", 0),
            "item#animal muscle oil#t0"
        );
    }
}
