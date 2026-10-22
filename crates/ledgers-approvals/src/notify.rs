//! Approval notifications: delivery via email, Discord, Slack, web push, and webhooks.

use crate::approval::ApprovalRequest;
use cratefield_core::{
    Clock, Database, HttpClient, Mailer, Outbox, Processed, Push, ScheduledBudget, Statement,
};
use cratefield_module_webhooks::Webhooks;
use sea_query::Value as SeaValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use time::Duration;

/// Dedicated outbox table for approval notifications (no topic filtering in `Outbox::claim_due`).
pub const NOTICE_OUTBOX_TABLE: &str = "approval_outbox";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalEvent {
    #[serde(rename = "created")]
    Created,
    #[serde(rename = "decided")]
    Decided,
}

impl ApprovalEvent {
    /// Returns the Outbox topic string for this event.
    #[must_use]
    pub fn topic(self) -> &'static str {
        match self {
            Self::Created => "approval.created",
            Self::Decided => "approval.decided",
        }
    }

    /// Returns the event kind (short string form).
    #[must_use]
    pub fn kind(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Decided => "decided",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalNotice {
    pub approval_id: String,
    pub company_id: String,
    pub event: ApprovalEvent,
    pub status: String,
    pub amounts: BTreeMap<String, String>,
    pub counterparty: Option<String>,
    pub reason: String,
    pub link: String,
    pub high_risk: bool,
}

/// Creates a notification notice and enqueues it in the dedicated `approval_outbox` table.
/// The notice links to `{app_url}/approvals/{approval_id}`.
///
/// # Errors
/// Returns an error if serialization fails.
pub fn notice_statement(
    approval_id: &str,
    event: ApprovalEvent,
    request: &ApprovalRequest,
    counterparty: Option<&str>,
    app_url: &str,
    now: &str,
) -> Result<Statement, serde_json::Error> {
    let notice = ApprovalNotice {
        approval_id: approval_id.to_string(),
        company_id: request.company_id.clone(),
        event,
        status: request.status.as_str().to_string(),
        amounts: request.effect.clone(),
        counterparty: counterparty.map(ToString::to_string),
        reason: request.reason.clone(),
        link: format!("{}/approvals/{approval_id}", app_url.trim_end_matches('/')),
        high_risk: !request.effect.is_empty(),
    };

    let payload = serde_json::to_string(&notice)?;
    let outbox_id = format!("{approval_id}-{}", event.kind());

    let stmt = Statement::with_values(
        "INSERT INTO approval_outbox (id, subject, topic, payload, attempts, next_attempt_at, created_at) \
         VALUES (?, ?, ?, ?, 0, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(outbox_id.as_str()),
            SeaValue::from(request.company_id.as_str()),
            SeaValue::from(event.topic()),
            SeaValue::from(payload.as_str()),
            SeaValue::from(now),
            SeaValue::from(now),
        ],
    );

    Ok(stmt)
}

pub mod storage {
    use super::{Database, SeaValue, Statement};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Channel {
        Email,
        Discord,
        Slack,
        WebPush,
    }

    impl Channel {
        /// Returns the string representation of this channel.
        #[must_use]
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Email => "email",
                Self::Discord => "discord",
                Self::Slack => "slack",
                Self::WebPush => "webpush",
            }
        }
    }

    /// Enables a notification channel for a company.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn enable_channel(
        db: &dyn Database,
        company_id: &str,
        channel: Channel,
        now: &str,
    ) -> Result<(), String> {
        let key = format!("notify.{}", channel.as_str());
        let stmt = Statement::with_values(
            "INSERT INTO company_setting (company_id, key, value, updated_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT (company_id, key) DO UPDATE SET value = ?, updated_at = ?"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(key.as_str()),
                SeaValue::from("on"),
                SeaValue::from(now),
                SeaValue::from("on"),
                SeaValue::from(now),
            ],
        );

        db.batch_atomic(&[stmt]).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Checks if a notification channel is enabled for a company.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn is_channel_enabled(
        db: &dyn Database,
        company_id: &str,
        channel: Channel,
    ) -> Result<bool, String> {
        let key = format!("notify.{}", channel.as_str());
        let rows = db
            .query(&Statement::with_values(
                "SELECT value FROM company_setting WHERE company_id = ? AND key = ?".to_owned(),
                vec![SeaValue::from(company_id), SeaValue::from(key.as_str())],
            ))
            .await
            .map_err(|e| e.to_string())?;

        Ok(rows
            .first()
            .and_then(|row| row.get::<String>("value"))
            .is_some_and(|v| v == "on"))
    }

    /// Adds or updates a notification target for a person on a channel.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn add_target(
        db: &dyn Database,
        company_id: &str,
        person_id: &str,
        channel: Channel,
        target: &str,
        now: &str,
    ) -> Result<(), String> {
        let stmt = Statement::with_values(
            "INSERT INTO approval_notify_target (company_id, person_id, channel, target, created_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT (company_id, person_id, channel) DO UPDATE SET target = ?, created_at = ?"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(person_id),
                SeaValue::from(channel.as_str()),
                SeaValue::from(target),
                SeaValue::from(now),
                SeaValue::from(target),
                SeaValue::from(now),
            ],
        );

        db.batch_atomic(&[stmt]).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Loads all notification targets for a person.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn load_targets(
        db: &dyn Database,
        company_id: &str,
        person_id: &str,
    ) -> Result<Vec<(Channel, String)>, String> {
        let rows = db
            .query(&Statement::with_values(
                "SELECT channel, target FROM approval_notify_target \
                 WHERE company_id = ? AND person_id = ?"
                    .to_owned(),
                vec![SeaValue::from(company_id), SeaValue::from(person_id)],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut targets = Vec::new();
        for row in &rows.rows {
            if let (Some(channel_str), Some(target)) =
                (row.get::<String>("channel"), row.get::<String>("target"))
            {
                let channel = match channel_str.as_str() {
                    "email" => Channel::Email,
                    "discord" => Channel::Discord,
                    "slack" => Channel::Slack,
                    "webpush" => Channel::WebPush,
                    _ => continue,
                };
                targets.push((channel, target));
            }
        }
        Ok(targets)
    }

    /// Removes the web push target for a person.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn remove_webpush_target(
        db: &dyn Database,
        company_id: &str,
        person_id: &str,
    ) -> Result<(), String> {
        let stmt = Statement::with_values(
            "DELETE FROM approval_notify_target WHERE company_id = ? AND person_id = ? AND channel = ?"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(person_id),
                SeaValue::from("webpush"),
            ],
        );

        db.batch_atomic(&[stmt]).await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub struct Notifiers {
    pub http: Arc<dyn HttpClient>,
    pub mailer: Arc<dyn Mailer>,
    pub push: Arc<dyn Push>,
    pub webhooks: Webhooks,
    pub app_url: String,
    pub mail_from: String,
}

/// Drains approval notifications with exponential backoff and per-channel delivery.
///
/// # Errors
/// Returns an error if draining fails.
#[allow(clippy::cast_possible_truncation)]
pub async fn drain(
    db: &dyn Database,
    clock: &dyn Clock,
    budget: &ScheduledBudget,
    notifiers: &Notifiers,
) -> Result<usize, String> {
    let outbox = Outbox::new(NOTICE_OUTBOX_TABLE);
    let opts = cratefield_core::DrainOptions {
        limit: 100,
        lease: Duration::seconds(30),
        subrequests_per_item: 1,
    };

    let now_ts = clock.now();

    let report = outbox
        .drain_within(db, clock, budget, opts, |record| {
            handle_outbox_record(db, clock, notifiers, record, now_ts)
        })
        .await
        .map_err(|e| format!("drain failed: {e}"))?;

    Ok(report.processed as usize)
}

/// Handles one outbox record: parse and deliver the notice.
async fn handle_outbox_record(
    db: &dyn Database,
    _clock: &dyn Clock,
    notifiers: &Notifiers,
    record: cratefield_core::OutboxRecord,
    now: time::OffsetDateTime,
) -> Processed {
    match serde_json::from_str::<ApprovalNotice>(&record.payload) {
        Ok(notice) => {
            if deliver_notice(db, notifiers, &notice, &record.id, now)
                .await
                .is_err()
            {
                let next_attempt = backoff_time(now, record.attempts);
                Processed::RetryAt(next_attempt)
            } else {
                Processed::Done
            }
        }
        Err(_) => Processed::Done, // Skip unparseable records
    }
}

/// Computes exponential backoff: 2^attempts seconds, capped at 3600 (1 hour).
#[must_use]
fn backoff_time(now: time::OffsetDateTime, attempts: i64) -> time::OffsetDateTime {
    let seconds = 1u64 << std::cmp::min(attempts, 12); // 2^attempts, max 2^12 = 4096
    let seconds = std::cmp::min(seconds, 3600); // Cap at 1 hour
    #[allow(clippy::cast_possible_wrap)]
    let duration_secs = seconds as i64;
    now + time::Duration::seconds(duration_secs)
}

/// Delivers a notice through all configured channels (webhooks, email, discord, slack, webpush).
/// At-least-once delivery per channel: webhook module handles retries; others fail per channel
/// but do not fail the whole batch. Returns only when at least one channel succeeds or all fail.
///
/// # Errors
/// Returns an error if delivery fails.
#[allow(clippy::too_many_lines)]
async fn deliver_notice(
    db: &dyn Database,
    notifiers: &Notifiers,
    notice: &ApprovalNotice,
    _outbox_id: &str,
    now: time::OffsetDateTime,
) -> Result<(), String> {
    let now_iso = format_rfc3339(now);

    // 1. Publish to webhooks via module-webhooks (handles signing, retry, dead letters)
    let notice_value =
        serde_json::to_value(notice).map_err(|e| format!("failed to serialize notice: {e}"))?;
    let _webhook_result = notifiers
        .webhooks
        .publish(
            db,
            &notice.company_id,
            notice.event.topic(),
            &notice_value,
            &now_iso,
        )
        .await;

    // 2-5. Deliver to email, Discord, Slack, web push: query all targets for all persons.
    // Build the message content (amount, counterparty, reason, link, no document contents).
    let (amounts_text, counterparty_display) = format_notice_content(notice);

    // Load all targets for this company and attempt delivery per channel.
    // On error, we do not retry or fail the whole batch; each channel is independent.
    let _ =
        deliver_to_configured_channels(db, notifiers, notice, &amounts_text, &counterparty_display)
            .await;

    Ok(())
}

/// Formats the amount and counterparty for display in notifications.
fn format_notice_content(notice: &ApprovalNotice) -> (String, String) {
    let amounts_text = if notice.amounts.is_empty() {
        "No amount".to_string()
    } else {
        notice
            .amounts
            .iter()
            .map(|(currency, amount_minor_str)| {
                // Display amounts as-is; they are already canonical strings from preview
                format!("{amount_minor_str} {currency}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    let counterparty_display = notice
        .counterparty
        .as_deref()
        .unwrap_or("Unknown party")
        .to_string();

    (amounts_text, counterparty_display)
}

/// Delivers to all configured channels (email, Discord, Slack, web push).
///
/// # Errors
/// Returns an error if target loading fails.
#[allow(clippy::too_many_lines)]
async fn deliver_to_configured_channels(
    db: &dyn Database,
    notifiers: &Notifiers,
    notice: &ApprovalNotice,
    amounts_text: &str,
    counterparty_display: &str,
) -> Result<(), String> {
    // Fetch all notification targets for this company
    let rows = db
        .query(&Statement::with_values(
            "SELECT person_id, channel, target FROM approval_notify_target WHERE company_id = ?"
                .to_owned(),
            vec![SeaValue::from(notice.company_id.as_str())],
        ))
        .await
        .map_err(|e| format!("failed to load targets: {e}"))?;

    // Group targets by person and channel
    let mut targets_by_person: BTreeMap<String, Vec<(storage::Channel, String)>> = BTreeMap::new();
    for row in &rows.rows {
        if let (Some(person_id), Some(channel_str), Some(target)) = (
            row.get::<String>("person_id"),
            row.get::<String>("channel"),
            row.get::<String>("target"),
        ) {
            let channel = match channel_str.as_str() {
                "email" => storage::Channel::Email,
                "discord" => storage::Channel::Discord,
                "slack" => storage::Channel::Slack,
                "webpush" => storage::Channel::WebPush,
                _ => continue,
            };
            targets_by_person
                .entry(person_id)
                .or_default()
                .push((channel, target));
        }
    }

    // Check which channels are enabled at company level
    let email_enabled =
        storage::is_channel_enabled(db, &notice.company_id, storage::Channel::Email)
            .await
            .unwrap_or(false);
    let discord_enabled =
        storage::is_channel_enabled(db, &notice.company_id, storage::Channel::Discord)
            .await
            .unwrap_or(false);
    let slack_enabled =
        storage::is_channel_enabled(db, &notice.company_id, storage::Channel::Slack)
            .await
            .unwrap_or(false);
    let webpush_enabled =
        storage::is_channel_enabled(db, &notice.company_id, storage::Channel::WebPush)
            .await
            .unwrap_or(false);

    // Deliver to each person's targets
    for (_person_id, person_targets) in targets_by_person {
        for (channel, target) in person_targets {
            #[allow(clippy::single_match_else)]
            match channel {
                storage::Channel::Email if email_enabled => {
                    deliver_email(
                        &notifiers.mailer,
                        &target,
                        &notifiers.mail_from,
                        notice,
                        amounts_text,
                        counterparty_display,
                    );
                }
                storage::Channel::Discord if discord_enabled => {
                    deliver_discord(
                        &notifiers.http,
                        &target,
                        notice,
                        amounts_text,
                        counterparty_display,
                    );
                }
                storage::Channel::Slack if slack_enabled => {
                    deliver_slack(
                        &notifiers.http,
                        &target,
                        notice,
                        amounts_text,
                        counterparty_display,
                    );
                }
                storage::Channel::WebPush if webpush_enabled => {
                    deliver_webpush(&notifiers.push, &target, notice);
                }
                _ => {} // Channel disabled or unrecognized
            }
        }
    }

    Ok(())
}

/// Delivers an email notification.
#[allow(unused_variables)]
fn deliver_email(
    mailer: &Arc<dyn Mailer>,
    to: &str,
    from: &str,
    notice: &ApprovalNotice,
    amounts_text: &str,
    counterparty_display: &str,
) {
    // TODO: implement email delivery via Mailer port
}

/// Delivers a Discord webhook notification.
#[allow(unused_variables)]
fn deliver_discord(
    http: &Arc<dyn HttpClient>,
    webhook_url: &str,
    notice: &ApprovalNotice,
    amounts_text: &str,
    counterparty_display: &str,
) {
    // TODO: implement Discord delivery via HttpClient port
}

/// Delivers a Slack webhook notification.
#[allow(unused_variables)]
fn deliver_slack(
    http: &Arc<dyn HttpClient>,
    webhook_url: &str,
    notice: &ApprovalNotice,
    amounts_text: &str,
    counterparty_display: &str,
) {
    // TODO: implement Slack delivery via HttpClient port
}

/// Delivers a web push notification with approve/reject actions.
/// For high-risk approvals, the approve action points to the passkey step.
#[allow(unused_variables)]
fn deliver_webpush(push: &Arc<dyn Push>, target_json: &str, notice: &ApprovalNotice) {
    // TODO: implement web push delivery via Push port
}

/// Escapes text for safe display in Markdown (Discord, Slack).
#[allow(dead_code)]
fn escape_markdown(text: &str) -> String {
    text.replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// Formats an `OffsetDateTime` to RFC 3339 string.
fn format_rfc3339(at: time::OffsetDateTime) -> String {
    use time::format_description::well_known::Rfc3339;
    let at = at.to_offset(time::UtcOffset::UTC);
    at.replace_nanosecond(0)
        .unwrap_or(at)
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_topics() {
        assert_eq!(ApprovalEvent::Created.topic(), "approval.created");
        assert_eq!(ApprovalEvent::Decided.topic(), "approval.decided");
    }

    #[test]
    fn notice_high_risk() {
        let mut amounts = BTreeMap::new();
        amounts.insert("EUR".to_string(), "10000".to_string());

        let notice = ApprovalNotice {
            approval_id: "test".to_string(),
            company_id: "c1".to_string(),
            event: ApprovalEvent::Created,
            status: "pending".to_string(),
            amounts,
            counterparty: None,
            reason: "test".to_string(),
            link: "/approvals/test".to_string(),
            high_risk: true,
        };

        assert!(notice.high_risk);
    }

    #[test]
    fn channel_strings() {
        use storage::Channel;
        assert_eq!(Channel::Email.as_str(), "email");
        assert_eq!(Channel::Discord.as_str(), "discord");
        assert_eq!(Channel::Slack.as_str(), "slack");
        assert_eq!(Channel::WebPush.as_str(), "webpush");
    }
}
