//! Reminders enqueued against invoices.
//!
//! Reminders never advance the invoice FSM — the invoice stays in
//! `Sent` or `Viewed`. The act of reminding is its own event class
//! (`inv.billing.reminder.scheduled` / `.sent`).

use super::ids::{InvoiceId, ReminderId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Delivery channel for a reminder.
///
/// Mirrors the inv delivery scheme (file/stdout/bus/webhook/link).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderChannel {
    /// Write the rendered PDF to a configured file path.
    File,
    /// Print the rendered PDF to stdout.
    Stdout,
    /// Emit `inv.billing.reminder.sent` on the bus; downstream consumer dispatches.
    Bus,
    /// POST JSON to a configured webhook URL.
    Webhook,
    /// Generate a signed shareable link.
    Link,
}

/// Lifecycle state of a [`Reminder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderState {
    /// Enqueued, not yet sent.
    Scheduled,
    /// Successfully dispatched.
    Sent,
    /// Cancelled before send — terminal.
    Cancelled,
}

/// A reminder to be sent against an invoice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    /// Stable identifier.
    pub id: ReminderId,
    /// Invoice the reminder targets.
    pub invoice_id: InvoiceId,
    /// When the ticker should dispatch this reminder.
    pub scheduled_at: DateTime<Utc>,
    /// When it actually got sent (`None` while `Scheduled` or `Cancelled`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<DateTime<Utc>>,
    /// How to deliver.
    pub channel: ReminderChannel,
    /// Lifecycle.
    pub state: ReminderState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_serde() {
        assert_eq!(serde_json::to_string(&ReminderChannel::Webhook).unwrap(), r#""webhook""#);
        assert_eq!(
            serde_json::from_str::<ReminderChannel>(r#""stdout""#).unwrap(),
            ReminderChannel::Stdout
        );
    }

    #[test]
    fn state_serde() {
        assert_eq!(serde_json::to_string(&ReminderState::Scheduled).unwrap(), r#""scheduled""#);
        assert_eq!(serde_json::to_string(&ReminderState::Cancelled).unwrap(), r#""cancelled""#);
    }

    #[test]
    fn reminder_round_trips() {
        let r = Reminder {
            id: ReminderId::new(),
            invoice_id: InvoiceId::new(),
            scheduled_at: Utc::now(),
            sent_at: None,
            channel: ReminderChannel::Webhook,
            state: ReminderState::Scheduled,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Reminder = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }
}
