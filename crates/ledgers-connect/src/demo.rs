//! The demo connections' vendor: a recorded fixture behind the vendor HTTP
//! port (issue #142). Demo connections hold no token. Their syncs read
//! `fixtures/demo/factory_zero.json` (invented figures shaped like the
//! vendors' answers) through the same connectors and the same command as a
//! real sync, so the books and the charts take the same path.
//!
//! The fixture is dated 2026. It is re-dated to the requested year when it is
//! served, so the demo book follows the current year like the rest of the demo.

use bytes::Bytes;
use cratefield_core::{HttpClient, HttpError};
use serde_json::{Map, Value, json};

const FIXTURE: &str = include_str!("../fixtures/demo/factory_zero.json");
const FIXTURE_YEAR: &str = "2026";

/// A vendor that answers from the demo fixture for one year.
pub struct DemoVendor {
    fixture: Value,
    year: String,
}

impl DemoVendor {
    /// The Factory Zero demo fixture, dated to `year`.
    ///
    /// # Panics
    /// Never: the fixture is part of the crate and is checked by its tests.
    #[must_use]
    pub fn factory_zero(year: i32) -> Self {
        Self {
            fixture: serde_json::from_str(FIXTURE).expect("the bundled demo fixture is JSON"),
            year: format!("{year:04}"),
        }
    }

    /// `2026-07` → `<year>-07`, for a period or a date the fixture stores.
    fn redate(&self, text: &str) -> String {
        match text.strip_prefix(FIXTURE_YEAR) {
            Some(rest) => format!("{}{rest}", self.year),
            None => text.to_owned(),
        }
    }

    /// Every string in a JSON value, dated to the requested year.
    fn redate_value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.redate(text)),
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.redate_value(item)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), self.redate_value(item)))
                    .collect::<Map<_, _>>(),
            ),
            other => other.clone(),
        }
    }

    fn answer(&self, url: &str, body: &str) -> Option<Value> {
        let root = &self.fixture;
        if url.contains("/billing/history") {
            let items = root.pointer("/cloudflare/billing")?;
            let result = self.redate_value(items);
            let count = result.as_array().map_or(0, Vec::len);
            return Some(json!({
                "success": true,
                "errors": [],
                "messages": [],
                "result": result,
                "result_info": {"count": count, "page": 1, "per_page": 50, "total_count": count},
            }));
        }
        if url.ends_with("/graphql") {
            let request: Value = serde_json::from_str(body).ok()?;
            let first = request.pointer("/variables/first")?.as_str()?;
            let month = first.get(5..7)?;
            let usage = root.pointer(&format!("/cloudflare/usage/{FIXTURE_YEAR}-{month}"))?;
            let workers: Vec<Value> = usage
                .get("workers")?
                .as_array()?
                .iter()
                .map(|row| json!({"dimensions": {"scriptName": row["script"]}, "sum": {"requests": row["requests"]}}))
                .collect();
            let d1: Vec<Value> = usage
                .get("d1")?
                .as_array()?
                .iter()
                .map(|row| json!({"dimensions": {"databaseId": row["database"]}, "sum": {"readQueries": row["reads"], "writeQueries": row["writes"]}}))
                .collect();
            return Some(json!({"data": {"viewer": {"accounts": [{
                "workersInvocationsAdaptive": workers,
                "d1AnalyticsAdaptiveGroups": d1,
            }]}}, "errors": null}));
        }
        if url.contains("/organizations/cost_report") {
            let cents = root.pointer("/anthropic/cost_cents")?.as_object()?;
            let data: Vec<Value> = cents
                .iter()
                .map(|(period, amount)| {
                    let start = self.redate(period);
                    json!({
                        "starting_at": format!("{start}-01T00:00:00Z"),
                        "ending_at": format!("{start}-02T00:00:00Z"),
                        "results": [{"amount": amount, "currency": "USD"}],
                    })
                })
                .collect();
            return Some(json!({"data": data, "has_more": false, "next_page": null}));
        }
        if url.contains("/organizations/usage_report/messages") {
            let usage = root.pointer("/anthropic/usage")?.as_object()?;
            let data: Vec<Value> = usage
                .iter()
                .map(|(period, keys)| {
                    let start = self.redate(period);
                    let results: Vec<Value> = keys
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|row| {
                            let api_key = if row["api_key_id"] == "console" { Value::Null } else { row["api_key_id"].clone() };
                            json!({
                                "api_key_id": api_key,
                                "uncached_input_tokens": row["tokens"],
                                "cache_read_input_tokens": 0,
                                "output_tokens": 0,
                                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0},
                            })
                        })
                        .collect();
                    json!({"starting_at": format!("{start}-01T00:00:00Z"), "results": results})
                })
                .collect();
            return Some(json!({"data": data, "has_more": false, "next_page": null}));
        }
        if url.contains("/invoices") {
            let data = self.redate_value(root.pointer("/planetscale/invoices")?);
            return Some(
                json!({"type": "list", "current_page": 1, "per_page": 100, "next_page": null, "data": data}),
            );
        }
        None
    }
}

#[async_trait::async_trait]
impl HttpClient for DemoVendor {
    async fn send(
        &self,
        request: http::Request<Bytes>,
    ) -> Result<http::Response<Bytes>, HttpError> {
        let url = request.uri().to_string();
        let body = String::from_utf8_lossy(request.body()).to_string();
        let (status, payload) = match self.answer(&url, &body) {
            Some(value) => (200, value.to_string()),
            None => (404, "{\"error\":\"not in the demo fixture\"}".to_owned()),
        };
        http::Response::builder()
            .status(status)
            .body(Bytes::from(payload))
            .map_err(|err| HttpError::Transport(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(vendor: &DemoVendor, url: &str, body: &str) -> (u16, Value) {
        let answer = vendor.answer(url, body);
        match answer {
            Some(value) => (200, value),
            None => (404, Value::Null),
        }
    }

    #[test]
    fn the_fixture_is_re_dated_to_the_requested_year() {
        let vendor = DemoVendor::factory_zero(2031);
        let (_, billing) = get(
            &vendor,
            "https://api.cloudflare.com/client/v4/accounts/x/billing/history",
            "",
        );
        let first = &billing["result"][0];
        assert!(
            first["occurred_at"]
                .as_str()
                .unwrap()
                .starts_with("2031-01-01")
        );
        assert_eq!(first["currency"], "SGD");
    }

    #[test]
    fn graphql_answers_the_month_asked_for() {
        let vendor = DemoVendor::factory_zero(2026);
        let body = json!({"variables": {"first": "2026-07-01"}}).to_string();
        let (status, answer) = get(
            &vendor,
            "https://api.cloudflare.com/client/v4/graphql",
            &body,
        );
        assert_eq!(status, 200);
        let accounts = &answer["data"]["viewer"]["accounts"][0];
        assert!(
            accounts["workersInvocationsAdaptive"]
                .as_array()
                .unwrap()
                .len()
                >= 10
        );
        assert_eq!(
            accounts["d1AnalyticsAdaptiveGroups"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn nothing_outside_the_fixture_is_answered() {
        let vendor = DemoVendor::factory_zero(2026);
        assert_eq!(get(&vendor, "https://api.example.test/other", "").0, 404);
    }

    #[test]
    fn every_month_has_usage_and_a_cost() {
        let root: Value = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(root["cloudflare"]["usage"].as_object().unwrap().len(), 12);
        assert_eq!(
            root["anthropic"]["cost_cents"].as_object().unwrap().len(),
            12
        );
        assert_eq!(
            root["planetscale"]["invoices"].as_array().unwrap().len(),
            12
        );
    }
}
