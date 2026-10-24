//! The Amazon Web Services connector (issue #142, cloud providers).
//!
//! Cost: Cost Explorer `GetCostAndUsage`, a JSON-protocol `POST` to
//! `https://ce.us-east-1.amazonaws.com/` with `X-Amz-Target:
//! AWSInsightsIndexService.GetCostAndUsage`. The request is `DAILY` granularity,
//! metric `UnblendedCost`, grouped by `SERVICE` and, when `tag_key` is set, by
//! that tag. Pages follow `NextPageToken`. Source:
//! <https://docs.aws.amazon.com/aws-cost-management/latest/APIReference/API_GetCostAndUsage.html>
//!
//! Allocation: by tag value when `tag_key` is configured (a resource with no
//! value books as `untagged`), otherwise by service. Costs sum per month and
//! currency, so one invoice per month and currency.
//!
//! Auth: AWS Signature Version 4, signed by [`crate::sigv4`]. The credential
//! is an IAM access key pair for a user whose only permission is
//! `ce:GetCostAndUsage`. The app help text carries that policy.

use crate::connector::{Connector, CredentialField, SyncContext, credential_text, next_day};
use crate::cost::{CostLine, micro_of, month_of, output};
use crate::http::VendorError;
use crate::model::SyncOutput;
use crate::sigv4::{AwsKeys, Target, amz_date, sign};
use async_trait::async_trait;
use serde_json::{Value, json};

const HOST: &str = "ce.us-east-1.amazonaws.com";
const HOSTS: &[&str] = &[HOST];
const ENDPOINT: &str = "https://ce.us-east-1.amazonaws.com/";
const REGION: &str = "us-east-1";
const SERVICE: &str = "ce";
const TARGET: &str = "AWSInsightsIndexService.GetCostAndUsage";
const CONTENT_TYPE: &str = "application/x-amz-json-1.1";
const MAX_PAGES: u32 = 50;

/// The AWS Cost Explorer connector.
pub struct AwsCostExplorer;

#[async_trait]
impl Connector for AwsCostExplorer {
    fn id(&self) -> &'static str {
        "aws"
    }

    fn display_name(&self) -> &'static str {
        "Amazon Web Services"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "access_key_id",
                label: "Access key ID (IAM user with ce:GetCostAndUsage only)",
            },
            CredentialField {
                name: "secret_access_key",
                label: "Secret access key",
            },
        ]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        tag_key(config).map(|_| ())
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let keys = AwsKeys {
            access_key_id: credential_text(ctx.credential, "access_key_id")
                .map_err(|detail| malformed(&detail))?,
            secret_access_key: credential_text(ctx.credential, "secret_access_key")
                .map_err(|detail| malformed(&detail))?,
        };
        let tag = tag_key(ctx.config).map_err(|detail| malformed(&detail))?;
        let end = next_day(ctx.until).ok_or_else(|| malformed("until is not a date"))?;
        let stamp = amz_date(ctx.now).ok_or_else(|| malformed("the clock is not RFC 3339"))?;
        let mut lines = Vec::new();
        let mut next: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut body = json!({
                "TimePeriod": {"Start": ctx.since, "End": end},
                "Granularity": "DAILY",
                "Metrics": ["UnblendedCost"],
                "GroupBy": group_by(tag.as_deref()),
            });
            if let Some(token) = &next {
                body["NextPageToken"] = json!(token);
            }
            let bytes = serde_json::to_vec(&body).map_err(|err| malformed(&err.to_string()))?;
            let signature = sign(
                &keys,
                &Target {
                    method: "POST",
                    region: REGION,
                    service: SERVICE,
                    host: HOST,
                },
                &stamp,
                &[("content-type", CONTENT_TYPE), ("x-amz-target", TARGET)],
                &bytes,
            );
            let headers = vec![
                ("X-Amz-Target", TARGET.to_owned()),
                ("X-Amz-Date", signature.amz_date),
                ("Authorization", signature.authorization),
            ];
            let answer = ctx
                .http
                .post_bytes(HOSTS, ENDPOINT, &headers, CONTENT_TYPE, bytes)
                .await?;
            parse_results(&answer, tag.is_some(), &mut lines)?;
            next = answer
                .get("NextPageToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .map(str::to_owned);
            if next.is_none() {
                break;
            }
        }
        let today = ctx.now.get(0..10).unwrap_or_default();
        let mut notes = Vec::new();
        let mut out = output("aws", "Amazon Web Services", &lines, today, &mut notes);
        if tag.is_some() {
            notes.push(
                "allocated by the tag value; a resource with no value books to the default unit as untagged"
                    .to_owned(),
            );
        } else {
            notes.push("allocated by AWS service; set tag_key to allocate by a tag".to_owned());
        }
        out.notes = notes;
        Ok(out)
    }
}

/// The optional tag key, checked: at most 128 printable characters, no `$`
/// (Cost Explorer uses it to separate the key from the value).
fn tag_key(config: &Value) -> Result<Option<String>, String> {
    match config.get("tag_key") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let key = text.trim();
            let printable = key.chars().all(|c| !c.is_control() && c != '$');
            if key.is_empty() || key.chars().count() > 128 || !printable {
                return Err("tag_key is a tag name of up to 128 characters without $".to_owned());
            }
            Ok(Some(key.to_owned()))
        }
        Some(_) => Err("tag_key is a tag name".to_owned()),
    }
}

fn group_by(tag: Option<&str>) -> Value {
    let mut groups = vec![json!({"Type": "DIMENSION", "Key": "SERVICE"})];
    if let Some(key) = tag {
        groups.push(json!({"Type": "TAG", "Key": key}));
    }
    json!(groups)
}

fn parse_results(
    answer: &Value,
    tagged: bool,
    lines: &mut Vec<CostLine>,
) -> Result<(), VendorError> {
    for result in answer
        .get("ResultsByTime")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let start = result
            .pointer("/TimePeriod/Start")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let period = month_of(start).ok_or_else(|| malformed("a result has no period"))?;
        for group in result
            .get("Groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let keys: Vec<&str> = group
                .get("Keys")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let metric = group
                .pointer("/Metrics/UnblendedCost")
                .ok_or_else(|| malformed("a group has no UnblendedCost"))?;
            let amount = metric
                .get("Amount")
                .and_then(Value::as_str)
                .ok_or_else(|| malformed("a cost has no amount string"))?;
            let currency = metric
                .get("Unit")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_uppercase();
            let micro = micro_of(amount).map_err(|err| malformed(&err.to_string()))?;
            let (kind, id) = if tagged {
                // `tagkey$value`; an untagged cost has an empty value.
                let value = keys
                    .get(1)
                    .and_then(|raw| raw.split_once('$'))
                    .map_or("", |(_, value)| value);
                let id = if value.is_empty() { "untagged" } else { value };
                ("tag", id.to_owned())
            } else {
                (
                    "service",
                    keys.first().copied().unwrap_or("unknown").to_owned(),
                )
            };
            lines.push(CostLine {
                period: period.clone(),
                currency,
                kind,
                name: id.clone(),
                id,
                micro,
            });
        }
    }
    Ok(())
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
