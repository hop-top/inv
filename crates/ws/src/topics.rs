//! Dotted-glob topic matcher used by the per-subscription forwarder.
//!
//! ## Grammar
//!
//! Patterns follow the dotted convention captured in design §11:
//!
//! - `*` matches exactly one segment.
//! - `#` matches zero-or-more trailing segments (and ONLY appears at the end).
//! - Anything else matches itself literally.
//!
//! `inv.billing.invoice.#` matches both `inv.billing.invoice.drafted`
//! and `inv.billing.invoice.payment.received`.

/// Test if `topic` matches a dotted glob `pattern`. See module docs for
/// the grammar (`*` segment-wild, `#` tail-wild, literals).
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    let pat_segs: Vec<&str> = pattern.split('.').collect();
    let top_segs: Vec<&str> = topic.split('.').collect();

    let mut i = 0;
    while i < pat_segs.len() {
        // `#` matches the rest of the topic (zero or more segments) — must be terminal.
        if pat_segs[i] == "#" {
            return i == pat_segs.len() - 1;
        }
        if i >= top_segs.len() {
            return false;
        }
        if pat_segs[i] != "*" && pat_segs[i] != top_segs[i] {
            return false;
        }
        i += 1;
    }
    // No `#` consumed the tail; the topic must have exactly as many segments.
    i == top_segs.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_literal_topic() {
        assert!(topic_matches(
            "inv.billing.invoice.drafted",
            "inv.billing.invoice.drafted"
        ));
        assert!(!topic_matches(
            "inv.billing.invoice.drafted",
            "inv.billing.invoice.issued"
        ));
    }

    #[test]
    fn matches_segment_wildcard() {
        assert!(topic_matches(
            "inv.billing.*.drafted",
            "inv.billing.invoice.drafted"
        ));
        assert!(!topic_matches(
            "inv.billing.*.drafted",
            "inv.billing.invoice.issued"
        ));
        // `*` matches exactly one segment, not zero.
        assert!(!topic_matches(
            "inv.billing.*.drafted",
            "inv.billing.drafted"
        ));
    }

    #[test]
    fn matches_tail_wildcard() {
        assert!(topic_matches(
            "inv.billing.invoice.#",
            "inv.billing.invoice.drafted"
        ));
        assert!(topic_matches(
            "inv.billing.invoice.#",
            "inv.billing.invoice.payment.received"
        ));
        // `#` at the end matches zero-or-more segments — including zero.
        assert!(topic_matches(
            "inv.billing.invoice.#",
            "inv.billing.invoice"
        ));
    }
}
