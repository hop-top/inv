//! `resolve_tax()` — the entry point of the tax engine.
//!
//! See design spec §6.1 for the algorithm. The function is pure: no I/O,
//! no clock, no global state — every input is on the parameter list.

use super::nexus::NexusConfig;
use super::table::TaxTable;
use crate::domain::address::Address;
use crate::domain::invoice::{InvoiceLine, TaxCategory};
use crate::domain::jurisdiction::Jurisdiction;
use crate::domain::money::Currency;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Buyer scope relative to the seller, per design §6.1 step 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuyerScope {
    /// Same country and same region as the seller.
    DomesticLocal,
    /// Same country, different region.
    DomesticOther,
    /// Different country.
    Export,
}

/// Output of [`resolve_tax`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTax {
    /// Tax amount on this line, rounded to the line currency's scale
    /// using banker's rounding.
    pub amount: Decimal,
    /// IDs of every tax-table row that contributed to `amount`. Recorded
    /// on the invoice line (`tax_rate_ids`) for audit.
    pub applied_rate_ids: Vec<String>,
    /// Set when the engine could not determine US economic nexus and the
    /// operator should review before sending.
    pub nexus_review: bool,
    /// True when the line crosses an international border (0 % applied,
    /// `export` flag on the rendered document).
    pub export: bool,
    /// Buyer scope inferred from the addresses.
    pub buyer_scope: BuyerScope,
    /// Non-fatal advisories (e.g. "reduced category requested but no
    /// reduced row exists; fell back to standard").
    pub warnings: Vec<String>,
}

/// Per-state revenue + transaction figures for the seller, evaluated
/// against [`NexusConfig`] during resolution.
///
/// v1 expects the caller (the command layer, eventually) to assemble
/// these numbers before invoking the resolver. The engine itself does
/// not aggregate ledger data.
#[derive(Debug, Clone, Default)]
pub struct NexusFigures {
    /// Seller's YTD (or threshold-window) revenue into the buyer's state.
    pub revenue: Decimal,
    /// Seller's transaction count into the buyer's state in the same window.
    pub txn_count: u32,
}

/// Errors from the tax resolver.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TaxError {
    /// Buyer country code is empty / not 2 letters / not understood.
    #[error("invalid buyer country code: {0:?}")]
    InvalidBuyerCountry(String),
    /// The tax table had no rows for an in-scope jurisdiction. Treated
    /// as a configuration error.
    #[error("no tax rates configured for seller {seller} (buyer scope {scope:?})")]
    NoRatesConfigured {
        /// Seller jurisdiction.
        seller: Jurisdiction,
        /// Inferred scope.
        scope: BuyerScope,
    },
}

/// Determine buyer scope from seller jurisdiction + buyer address.
fn classify(seller: Jurisdiction, buyer: &Address) -> Result<BuyerScope, TaxError> {
    if buyer.country.is_empty() {
        return Err(TaxError::InvalidBuyerCountry(buyer.country.clone()));
    }
    let same_country = buyer.country.eq_ignore_ascii_case(seller.country());
    if !same_country {
        return Ok(BuyerScope::Export);
    }
    let seller_region = match seller {
        Jurisdiction::QuebecCa => "QC",
        Jurisdiction::DelawareUs => "DE",
        Jurisdiction::AlgiersDz => "16",
    };
    match buyer.region.as_deref() {
        Some(r) if r.eq_ignore_ascii_case(seller_region) => Ok(BuyerScope::DomesticLocal),
        Some(_) => Ok(BuyerScope::DomesticOther),
        // No region: treat as same-region for DZ (TVA is national); for
        // CA and US the engine can't infer the destination, so the
        // safest interpretation is "domestic_other" (caller's address
        // wasn't specific enough for sub-national tax).
        None => match seller {
            Jurisdiction::AlgiersDz => Ok(BuyerScope::DomesticLocal),
            _ => Ok(BuyerScope::DomesticOther),
        },
    }
}

/// Pick effective rows for a line, taking the tax_category override into
/// account. Returns (rates, warnings).
fn pick_rates<'a>(
    seller: Jurisdiction,
    buyer: &Address,
    category: TaxCategory,
    table: &'a TaxTable,
    at_date: chrono::NaiveDate,
) -> (Vec<&'a super::rate::TaxRate>, Vec<String>) {
    let mut warnings = Vec::new();
    let buyer_country = &buyer.country;
    let buyer_region = buyer.region.as_deref();

    let lookup_with = |cat| table.lookup(seller, buyer_country, buyer_region, cat, at_date);

    match category {
        TaxCategory::Standard => (lookup_with(TaxCategory::Standard), warnings),
        TaxCategory::Reduced => {
            let reduced = lookup_with(TaxCategory::Reduced);
            if reduced.is_empty() {
                warnings.push(format!(
                    "reduced rate requested for seller {seller} -> buyer {buyer_country}/{region:?}; no reduced row found, falling back to standard",
                    region = buyer_region.unwrap_or("-"),
                ));
                (lookup_with(TaxCategory::Standard), warnings)
            } else {
                (reduced, warnings)
            }
        }
        TaxCategory::ZeroRated => {
            // Zero-rated: 0 %, but record an explicit row id if one exists
            // (matches design §6.1 step 3: "rate ID recorded").
            let zr = lookup_with(TaxCategory::ZeroRated);
            (zr, warnings)
        }
        TaxCategory::Exempt => {
            let ex = lookup_with(TaxCategory::Exempt);
            (ex, warnings)
        }
    }
}

/// Resolve the tax for a single invoice line.
///
/// Inputs:
/// - `seller`: seller jurisdiction
/// - `buyer`: buyer postal address (country required; region recommended)
/// - `line`: the invoice line (qty, unit price, category)
/// - `currency`: invoice currency, used for banker's rounding scale
/// - `table`: loaded tax table
/// - `nexus`: loaded nexus config
/// - `nexus_figures`: seller's per-state revenue + txn count (only
///   consulted when buyer is in a US state other than DE)
/// - `at`: when the rate lookup is evaluated (typically `Utc::now()`)
#[allow(clippy::too_many_arguments)]
pub fn resolve_tax(
    seller: Jurisdiction,
    buyer: &Address,
    line: &InvoiceLine,
    currency: Currency,
    table: &TaxTable,
    nexus: &NexusConfig,
    nexus_figures: &NexusFigures,
    at: DateTime<Utc>,
) -> Result<ResolvedTax, TaxError> {
    let scope = classify(seller, buyer)?;
    let at_date = at.date_naive();
    let base = line.quantity * line.unit_price;
    let zero = ResolvedTax {
        amount: currency.round(Decimal::ZERO),
        applied_rate_ids: Vec::new(),
        nexus_review: false,
        export: false,
        buyer_scope: scope,
        warnings: Vec::new(),
    };

    // Step 1: handle category overrides that short-circuit to 0%.
    match line.tax_category {
        TaxCategory::ZeroRated | TaxCategory::Exempt => {
            // Try to record a matching row id for audit (per §6.1).
            let (matches, warnings) =
                pick_rates(seller, buyer, line.tax_category, table, at_date);
            return Ok(ResolvedTax {
                amount: currency.round(Decimal::ZERO),
                applied_rate_ids: matches.iter().map(|r| r.id.clone()).collect(),
                nexus_review: false,
                export: matches!(scope, BuyerScope::Export),
                buyer_scope: scope,
                warnings,
            });
        }
        _ => {}
    }

    // Step 2: by scope.
    match scope {
        BuyerScope::Export => Ok(ResolvedTax { export: true, ..zero }),

        BuyerScope::DomesticLocal => {
            let (rates, warnings) =
                pick_rates(seller, buyer, line.tax_category, table, at_date);
            if rates.is_empty() {
                // No table rows means 0% with a warning, not an error —
                // operators can run inv without configuring rates yet.
                let mut w = warnings;
                w.push(format!(
                    "no tax rows for seller {seller} -> domestic-local buyer {country}/{region:?}; defaulting to 0%",
                    country = buyer.country,
                    region = buyer.region.as_deref().unwrap_or("-"),
                ));
                return Ok(ResolvedTax {
                    amount: currency.round(Decimal::ZERO),
                    applied_rate_ids: Vec::new(),
                    nexus_review: false,
                    export: false,
                    buyer_scope: scope,
                    warnings: w,
                });
            }
            let mut amount = Decimal::ZERO;
            let mut ids = Vec::with_capacity(rates.len());
            for r in &rates {
                amount += base * r.rate;
                ids.push(r.id.clone());
            }
            Ok(ResolvedTax {
                amount: currency.round(amount),
                applied_rate_ids: ids,
                nexus_review: false,
                export: false,
                buyer_scope: scope,
                warnings,
            })
        }

        BuyerScope::DomesticOther => match seller {
            // CA seller, CA buyer (non-QC): destination province via table.
            Jurisdiction::QuebecCa => {
                let (rates, warnings) =
                    pick_rates(seller, buyer, line.tax_category, table, at_date);
                let mut amount = Decimal::ZERO;
                let mut ids = Vec::with_capacity(rates.len());
                for r in &rates {
                    amount += base * r.rate;
                    ids.push(r.id.clone());
                }
                Ok(ResolvedTax {
                    amount: currency.round(amount),
                    applied_rate_ids: ids,
                    nexus_review: false,
                    export: false,
                    buyer_scope: scope,
                    warnings,
                })
            }
            // US seller, US buyer (non-DE): nexus gate.
            Jurisdiction::DelawareUs => {
                let buyer_region = match buyer.region.as_deref() {
                    Some(r) => r,
                    None => {
                        // Can't run nexus without region; flag review.
                        return Ok(ResolvedTax {
                            amount: currency.round(Decimal::ZERO),
                            applied_rate_ids: Vec::new(),
                            nexus_review: true,
                            export: false,
                            buyer_scope: scope,
                            warnings: vec![
                                "US-DE seller / US buyer: no region on address; cannot evaluate nexus, flagging review".into(),
                            ],
                        });
                    }
                };
                let threshold = nexus.get(buyer_region);
                let has_nexus = threshold
                    .map(|t| t.has_nexus(nexus_figures.revenue, nexus_figures.txn_count))
                    .unwrap_or(false);
                if has_nexus {
                    let (rates, warnings) =
                        pick_rates(seller, buyer, line.tax_category, table, at_date);
                    let mut amount = Decimal::ZERO;
                    let mut ids = Vec::with_capacity(rates.len());
                    for r in &rates {
                        amount += base * r.rate;
                        ids.push(r.id.clone());
                    }
                    let mut w = warnings;
                    if rates.is_empty() {
                        w.push(format!(
                            "US-DE seller / US-{buyer_region} buyer: nexus crossed but no rate row found"
                        ));
                    }
                    Ok(ResolvedTax {
                        amount: currency.round(amount),
                        applied_rate_ids: ids,
                        nexus_review: false,
                        export: false,
                        buyer_scope: scope,
                        warnings: w,
                    })
                } else {
                    // No nexus -> 0% with review flag (operator decides
                    // whether enabling/configuring nexus is right).
                    let reason = if let Some(t) = threshold {
                        if t.enabled {
                            "nexus enabled but threshold not crossed"
                        } else {
                            "nexus disabled for this state"
                        }
                    } else {
                        "no nexus row configured for this state"
                    };
                    Ok(ResolvedTax {
                        amount: currency.round(Decimal::ZERO),
                        applied_rate_ids: Vec::new(),
                        nexus_review: true,
                        export: false,
                        buyer_scope: scope,
                        warnings: vec![format!(
                            "US-DE seller / US-{buyer_region} buyer: {reason}; 0% applied"
                        )],
                    })
                }
            }
            // DZ seller, DZ buyer (non-Algiers): treat as domestic_local
            // (TVA is national). classify() already routes this to
            // DomesticLocal when region is absent; this arm only fires
            // when the buyer's region is set and != "16".
            Jurisdiction::AlgiersDz => {
                let (rates, warnings) =
                    pick_rates(seller, buyer, line.tax_category, table, at_date);
                let mut amount = Decimal::ZERO;
                let mut ids = Vec::with_capacity(rates.len());
                for r in &rates {
                    amount += base * r.rate;
                    ids.push(r.id.clone());
                }
                Ok(ResolvedTax {
                    amount: currency.round(amount),
                    applied_rate_ids: ids,
                    nexus_review: false,
                    export: false,
                    buyer_scope: scope,
                    warnings,
                })
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ids::{InvoiceId, LineId};
    use chrono::TimeZone;
    use rust_decimal::prelude::FromStr;
    use std::collections::BTreeMap;

    fn line_qty_price(qty: &str, price: &str, category: TaxCategory) -> InvoiceLine {
        InvoiceLine {
            id: LineId::new(),
            invoice_id: InvoiceId::new(),
            position: 0,
            description: "test".into(),
            quantity: Decimal::from_str(qty).unwrap(),
            unit_price: Decimal::from_str(price).unwrap(),
            tax_rate_ids: Vec::new(),
            tax_category: category,
            tax_amount: Decimal::ZERO,
            line_total: Decimal::ZERO,
            metadata: BTreeMap::new(),
        }
    }

    fn fixture_table() -> (TaxTable, NexusConfig) {
        let toml_text = r#"
[[rate]]
id = "ca-qc-gst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA" }
name = "GST"
category = "standard"
rate = "0.05"
effective_from = "2008-01-01"

[[rate]]
id = "ca-qc-qst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA", region = "QC" }
name = "QST"
category = "standard"
rate = "0.09975"
effective_from = "2013-01-01"

[[rate]]
id = "ca-on-hst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA", region = "ON" }
name = "HST (ON)"
category = "standard"
rate = "0.13"
effective_from = "2010-07-01"

[[rate]]
id = "ca-qc-gst-zero-foods"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA" }
name = "GST zero-rated (basic groceries)"
category = "zero_rated"
rate = "0.00"
effective_from = "1991-01-01"

[[rate]]
id = "dz-tva-standard"
jurisdiction = "DZ-16"
applies_to_buyer = { country = "DZ" }
name = "TVA"
category = "standard"
rate = "0.19"
effective_from = "2017-01-01"

[[rate]]
id = "dz-tva-reduced"
jurisdiction = "DZ-16"
applies_to_buyer = { country = "DZ" }
name = "TVA reduite"
category = "reduced"
rate = "0.09"
effective_from = "2017-01-01"

[[rate]]
id = "us-ca-sales"
jurisdiction = "US-DE"
applies_to_buyer = { country = "US", region = "CA" }
name = "CA Sales Tax"
category = "standard"
rate = "0.0725"
effective_from = "2017-01-01"

[nexus.CA]
enabled = true
revenue = "500000"
txn_count = 200

[nexus.NY]
enabled = false
revenue = "500000"
"#;
        TaxTable::load_from_str(toml_text).unwrap()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap()
    }

    // ----- classification -------------------------------------------------

    #[test]
    fn classify_qc_qc_is_domestic_local() {
        let buyer = Address {
            country: "CA".into(),
            region: Some("QC".into()),
            ..Address::country_only("CA")
        };
        assert_eq!(
            classify(Jurisdiction::QuebecCa, &buyer).unwrap(),
            BuyerScope::DomesticLocal
        );
    }

    #[test]
    fn classify_qc_on_is_domestic_other() {
        let buyer = Address {
            country: "CA".into(),
            region: Some("ON".into()),
            ..Address::country_only("CA")
        };
        assert_eq!(
            classify(Jurisdiction::QuebecCa, &buyer).unwrap(),
            BuyerScope::DomesticOther
        );
    }

    #[test]
    fn classify_qc_dz_is_export() {
        let buyer = Address::country_only("DZ");
        assert_eq!(
            classify(Jurisdiction::QuebecCa, &buyer).unwrap(),
            BuyerScope::Export
        );
    }

    #[test]
    fn classify_dz_no_region_is_domestic_local() {
        let buyer = Address::country_only("DZ");
        assert_eq!(
            classify(Jurisdiction::AlgiersDz, &buyer).unwrap(),
            BuyerScope::DomesticLocal
        );
    }

    #[test]
    fn classify_empty_country_errors() {
        let buyer = Address {
            country: String::new(),
            ..Address::country_only("")
        };
        assert!(matches!(
            classify(Jurisdiction::QuebecCa, &buyer),
            Err(TaxError::InvalidBuyerCountry(_))
        ));
    }

    // ----- QC seller permutations ----------------------------------------

    #[test]
    fn qc_seller_qc_buyer_gst_plus_qst() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "CA".into(),
            region: Some("QC".into()),
            ..Address::country_only("CA")
        };
        let line = line_qty_price("10", "125.00", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::QuebecCa,
            &buyer,
            &line,
            Currency::CAD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        // 10 * 125 = 1250. GST 5% = 62.50, QST 9.975% = 124.6875.
        // Sum = 187.1875 -> banker's round to 2 dp = 187.19 (5 rounds up
        // to 9 since 8 is even -> next even is 8 doesn't apply, 187.1875
        // -> the digit dropped is 75; we are at scale 2 so we drop 75
        // from .1875 giving us .18 + carry from .0075 -> 75 hundredths
        // of a hundredth; banker's-roundo .1875 to 2 places: midpoint
        // between .18 and .19 is .185 — .1875 is past .185, rounds up
        // to .19. Result .19.
        assert_eq!(r.amount, Decimal::from_str("187.19").unwrap());
        assert_eq!(r.applied_rate_ids.len(), 2);
        assert!(r.applied_rate_ids.iter().any(|i| i == "ca-qc-gst"));
        assert!(r.applied_rate_ids.iter().any(|i| i == "ca-qc-qst"));
        assert!(!r.nexus_review);
        assert!(!r.export);
        assert_eq!(r.buyer_scope, BuyerScope::DomesticLocal);
    }

    #[test]
    fn qc_seller_on_buyer_hst() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "CA".into(),
            region: Some("ON".into()),
            ..Address::country_only("CA")
        };
        let line = line_qty_price("4", "100", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::QuebecCa,
            &buyer,
            &line,
            Currency::CAD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        // 400 * (GST 5% + HST(ON) 13%) = 400 * 0.18 = 72.00
        assert_eq!(r.amount, Decimal::from_str("72.00").unwrap());
        assert_eq!(r.buyer_scope, BuyerScope::DomesticOther);
    }

    #[test]
    fn qc_seller_us_buyer_export_zero() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "US".into(),
            region: Some("CA".into()),
            ..Address::country_only("US")
        };
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::QuebecCa,
            &buyer,
            &line,
            Currency::USD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::from_str("0.00").unwrap());
        assert!(r.export);
        assert_eq!(r.buyer_scope, BuyerScope::Export);
    }

    // ----- DZ seller permutations ----------------------------------------

    #[test]
    fn dz_seller_dz_buyer_standard_tva() {
        let (table, nexus) = fixture_table();
        let buyer = Address::country_only("DZ");
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::AlgiersDz,
            &buyer,
            &line,
            Currency::DZD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        // 1000 * 0.19 = 190; DZD scale=0 so no change.
        assert_eq!(r.amount, Decimal::from_str("190").unwrap());
        assert_eq!(r.applied_rate_ids, vec!["dz-tva-standard".to_string()]);
    }

    #[test]
    fn dz_seller_dz_buyer_other_region_still_tva() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "DZ".into(),
            region: Some("31".into()), // Oran
            ..Address::country_only("DZ")
        };
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::AlgiersDz,
            &buyer,
            &line,
            Currency::DZD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::from_str("190").unwrap());
        // DZ logic: domestic_other -> still applies TVA because the
        // rate row has country=DZ with no region filter.
        assert_eq!(r.buyer_scope, BuyerScope::DomesticOther);
    }

    #[test]
    fn dz_seller_reduced_category() {
        let (table, nexus) = fixture_table();
        let buyer = Address::country_only("DZ");
        let line = line_qty_price("1", "1000", TaxCategory::Reduced);
        let r = resolve_tax(
            Jurisdiction::AlgiersDz,
            &buyer,
            &line,
            Currency::DZD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::from_str("90").unwrap());
        assert_eq!(r.applied_rate_ids, vec!["dz-tva-reduced".to_string()]);
    }

    #[test]
    fn dz_seller_export_to_ca_zero() {
        let (table, nexus) = fixture_table();
        let buyer = Address::country_only("CA");
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::AlgiersDz,
            &buyer,
            &line,
            Currency::DZD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::ZERO);
        assert!(r.export);
    }

    // ----- US-DE seller permutations -------------------------------------

    #[test]
    fn us_seller_us_buyer_nexus_on_threshold_crossed() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "US".into(),
            region: Some("CA".into()),
            ..Address::country_only("US")
        };
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let figs = NexusFigures {
            revenue: Decimal::from_str("600000").unwrap(),
            txn_count: 0,
        };
        let r = resolve_tax(
            Jurisdiction::DelawareUs,
            &buyer,
            &line,
            Currency::USD,
            &table,
            &nexus,
            &figs,
            now(),
        )
        .unwrap();
        // 1000 * 0.0725 = 72.50
        assert_eq!(r.amount, Decimal::from_str("72.50").unwrap());
        assert_eq!(r.applied_rate_ids, vec!["us-ca-sales".to_string()]);
        assert!(!r.nexus_review);
    }

    #[test]
    fn us_seller_us_buyer_nexus_on_threshold_not_crossed() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "US".into(),
            region: Some("CA".into()),
            ..Address::country_only("US")
        };
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let figs = NexusFigures {
            revenue: Decimal::from_str("10000").unwrap(),
            txn_count: 5,
        };
        let r = resolve_tax(
            Jurisdiction::DelawareUs,
            &buyer,
            &line,
            Currency::USD,
            &table,
            &nexus,
            &figs,
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::ZERO);
        assert!(r.nexus_review);
        assert!(r.applied_rate_ids.is_empty());
    }

    #[test]
    fn us_seller_us_buyer_nexus_disabled_state() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "US".into(),
            region: Some("NY".into()),
            ..Address::country_only("US")
        };
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let figs = NexusFigures {
            revenue: Decimal::from_str("999999999").unwrap(),
            txn_count: 9999,
        };
        let r = resolve_tax(
            Jurisdiction::DelawareUs,
            &buyer,
            &line,
            Currency::USD,
            &table,
            &nexus,
            &figs,
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::ZERO);
        assert!(r.nexus_review);
    }

    #[test]
    fn us_seller_us_buyer_de_is_domestic_local() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "US".into(),
            region: Some("DE".into()),
            ..Address::country_only("US")
        };
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::DelawareUs,
            &buyer,
            &line,
            Currency::USD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        // Delaware has no sales tax; no row -> 0% with warning.
        assert_eq!(r.amount, Decimal::ZERO);
        assert_eq!(r.buyer_scope, BuyerScope::DomesticLocal);
        assert!(!r.warnings.is_empty());
    }

    #[test]
    fn us_seller_export_to_ca() {
        let (table, nexus) = fixture_table();
        let buyer = Address::country_only("CA");
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::DelawareUs,
            &buyer,
            &line,
            Currency::USD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::ZERO);
        assert!(r.export);
    }

    #[test]
    fn us_seller_us_buyer_no_region_flags_review() {
        let (table, nexus) = fixture_table();
        let buyer = Address::country_only("US");
        let line = line_qty_price("1", "1000", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::DelawareUs,
            &buyer,
            &line,
            Currency::USD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert!(r.nexus_review);
        assert_eq!(r.amount, Decimal::ZERO);
    }

    // ----- Category overrides --------------------------------------------

    #[test]
    fn zero_rated_short_circuits_to_zero_with_recorded_id() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "CA".into(),
            region: Some("QC".into()),
            ..Address::country_only("CA")
        };
        let line = line_qty_price("1", "1000", TaxCategory::ZeroRated);
        let r = resolve_tax(
            Jurisdiction::QuebecCa,
            &buyer,
            &line,
            Currency::CAD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::ZERO);
        // The zero-rated row from the fixture should be recorded.
        assert_eq!(
            r.applied_rate_ids,
            vec!["ca-qc-gst-zero-foods".to_string()]
        );
    }

    #[test]
    fn exempt_short_circuits_to_zero() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "CA".into(),
            region: Some("QC".into()),
            ..Address::country_only("CA")
        };
        let line = line_qty_price("1", "1000", TaxCategory::Exempt);
        let r = resolve_tax(
            Jurisdiction::QuebecCa,
            &buyer,
            &line,
            Currency::CAD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::ZERO);
    }

    #[test]
    fn reduced_falls_back_to_standard_with_warning_when_no_reduced_row() {
        let (table, nexus) = fixture_table();
        // QC has no reduced row in the fixture; CA-buyer in QC should
        // fall back to GST+QST standard with a warning.
        let buyer = Address {
            country: "CA".into(),
            region: Some("QC".into()),
            ..Address::country_only("CA")
        };
        let line = line_qty_price("10", "125", TaxCategory::Reduced);
        let r = resolve_tax(
            Jurisdiction::QuebecCa,
            &buyer,
            &line,
            Currency::CAD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::from_str("187.19").unwrap());
        assert!(r.warnings.iter().any(|w| w.contains("reduced")));
    }

    // ----- Rounding ------------------------------------------------------

    #[test]
    fn bankers_rounding_usd_cad_scale_two() {
        let (table, nexus) = fixture_table();
        let buyer = Address {
            country: "US".into(),
            region: Some("CA".into()),
            ..Address::country_only("US")
        };
        let line = line_qty_price("1", "12.345", TaxCategory::Standard);
        let figs = NexusFigures {
            revenue: Decimal::from_str("600000").unwrap(),
            txn_count: 0,
        };
        let r = resolve_tax(
            Jurisdiction::DelawareUs,
            &buyer,
            &line,
            Currency::USD,
            &table,
            &nexus,
            &figs,
            now(),
        )
        .unwrap();
        // 12.345 * 0.0725 = 0.89500125 -> 2 dp banker's = 0.90 (round
        // 0.895 midpoint to even hundredth 0.90, .89 is odd .90 is even).
        // 0.89500125 isn't exactly 0.895 so it rounds simply up.
        // assert scale-2 rendering.
        assert_eq!(r.amount.scale(), 2);
    }

    #[test]
    fn bankers_rounding_dzd_scale_zero() {
        let (table, nexus) = fixture_table();
        let buyer = Address::country_only("DZ");
        // 10.5 base * 0.19 = 1.995 -> 0 dp banker's = 2.
        let line = line_qty_price("1", "10.5", TaxCategory::Standard);
        let r = resolve_tax(
            Jurisdiction::AlgiersDz,
            &buyer,
            &line,
            Currency::DZD,
            &table,
            &nexus,
            &NexusFigures::default(),
            now(),
        )
        .unwrap();
        assert_eq!(r.amount, Decimal::from_str("2").unwrap());
        assert_eq!(r.amount.scale(), 0);
    }

    #[test]
    fn bankers_rounding_midpoint_to_even() {
        // Currency::round uses MidpointNearestEven. Verify via a direct
        // ratio that yields exactly N.5 at scale-0:
        //  base 10, rate 0.05 -> 0.50 -> DZD round -> 0 (even).
        //  base 30, rate 0.05 -> 1.50 -> DZD round -> 2 (even).
        let (table, nexus) = fixture_table();
        let buyer = Address::country_only("DZ");

        // Replace fixture with a 5% rate via direct call: we already have
        // 19% TVA. Use Currency::round in isolation instead — verifies
        // the same Decimal::round_dp_with_strategy contract end-to-end.
        let _ = (table, nexus, buyer);
        assert_eq!(
            Currency::DZD.round(Decimal::from_str("0.5").unwrap()),
            Decimal::from_str("0").unwrap()
        );
        assert_eq!(
            Currency::DZD.round(Decimal::from_str("1.5").unwrap()),
            Decimal::from_str("2").unwrap()
        );
        assert_eq!(
            Currency::USD.round(Decimal::from_str("0.125").unwrap()),
            Decimal::from_str("0.12").unwrap()
        );
        assert_eq!(
            Currency::USD.round(Decimal::from_str("0.135").unwrap()),
            Decimal::from_str("0.14").unwrap()
        );
    }
}
