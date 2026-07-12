# The observation store: one on-demand table holding both observation rows
# (pk "item#<name>#t<tier>", sk "obs#<observedAt>#<contributor>") and
# contributor records (pk "contributor#<id>", sk "meta").
#
# Contributor rows are data, not infrastructure: they hold a token digest,
# and secrets do not belong in Terraform state. They are minted from the
# runbook (see ../README.md), never here.
#
# Point-in-time recovery stays off deliberately: every observation is
# re-submittable public market data, and PITR would be the table's largest
# cost line. Observations age out via the expiresAt TTL attribute.

resource "aws_dynamodb_table" "observations" {
  name         = var.table_name
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "pk"
  range_key    = "sk"

  attribute {
    name = "pk"
    type = "S"
  }

  attribute {
    name = "sk"
    type = "S"
  }

  ttl {
    attribute_name = "expiresAt"
    enabled        = true
  }

  deletion_protection_enabled = true
}
