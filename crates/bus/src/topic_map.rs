//! Config-driven topic remap (design §4.3).
//!
//! Deployments map upstream topics (e.g. the eventual `fin.*` shapes)
//! onto the canonical names `inv` consumes. The map is a pure
//! `BTreeMap<String, String>` loaded from the `[bus.remap]` block in
//! config; lookups are exact-match.
//!
//! Pattern-matching (`*` / `#` from kit's MQTT-style subscription
//! syntax) is NOT applied here — that's the subscription layer's job.
//! Remap is a string-to-string canonicaliser, evaluated once per
//! inbound event before dispatching.

use std::collections::BTreeMap;

/// Config-driven topic remap.
///
/// Built from the `[bus.remap]` block in inv's config TOML — typically
///
/// ```toml
/// [bus.remap]
/// "fin.finance.charge.created" = "fin.billing.charge.created"
/// ```
///
/// Lookups are exact-match against the input topic; misses pass the
/// input through unchanged (see [`remap_topic`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TopicMap {
    entries: BTreeMap<String, String>,
}

impl TopicMap {
    /// Construct an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct from a (topic, canonical) iterator.
    pub fn from_pairs<I, A, B>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (A, B)>,
        A: Into<String>,
        B: Into<String>,
    {
        let entries = pairs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        Self { entries }
    }

    /// Add or replace one entry.
    pub fn insert(&mut self, topic: impl Into<String>, canonical: impl Into<String>) {
        self.entries.insert(topic.into(), canonical.into());
    }

    /// Total entry count.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up an entry. Returns `None` on a miss.
    pub fn get(&self, topic: &str) -> Option<&str> {
        self.entries.get(topic).map(|s| s.as_str())
    }
}

/// Apply the remap. Returns the canonical name when one is configured,
/// or the input verbatim when no entry matches.
///
/// The function is pure (no async, no I/O) so router code can call it
/// inline ahead of dispatch.
pub fn remap_topic<'t>(map: &'t TopicMap, topic: &'t str) -> &'t str {
    map.get(topic).unwrap_or(topic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_map_passes_topic_through() {
        let map = TopicMap::new();
        assert_eq!(remap_topic(&map, "fin.billing.charge.created"), "fin.billing.charge.created");
    }

    #[test]
    fn configured_entry_remaps() {
        let map = TopicMap::from_pairs([(
            "fin.finance.charge.created",
            "fin.billing.charge.created",
        )]);
        assert_eq!(
            remap_topic(&map, "fin.finance.charge.created"),
            "fin.billing.charge.created"
        );
    }

    #[test]
    fn unmapped_topic_passes_through() {
        let map = TopicMap::from_pairs([(
            "fin.finance.charge.created",
            "fin.billing.charge.created",
        )]);
        assert_eq!(remap_topic(&map, "other.topic"), "other.topic");
    }
}
