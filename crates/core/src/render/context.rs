//! [`RenderContext`] — the bundle of inputs an invoice template sees.
//!
//! Templates address fields via the Tera-flavoured JSON shape produced by
//! [`RenderContext::to_tera_context`]. That shape is intentionally flat
//! enough to be stable across template authors: top-level `invoice`,
//! `customer`, `lines`, plus a few computed convenience fields.
//!
//! Authors should treat the JSON shape as the contract. Adding new
//! computed fields is backward-compatible; renaming or removing fields
//! is not.

use crate::domain::customer::Customer;
use crate::domain::invoice::{Invoice, InvoiceLine};
use serde::Serialize;
use tera::Context as TeraContext;

/// Inputs bundled for the invoice template.
///
/// Construct one per-render. Cheap to build; clones the supplied
/// references into owned values for serialization.
#[derive(Debug, Clone)]
pub struct RenderContext {
    /// Invoice being rendered.
    pub invoice: Invoice,
    /// Bill-to customer (must match `invoice.customer_id`; not enforced
    /// here — the caller is the source of truth).
    pub customer: Customer,
    /// Invoice lines (sorted by `position` ascending by convention).
    pub lines: Vec<InvoiceLine>,
}

/// Shape exposed to templates.
///
/// Kept in this module so the JSON keys are reviewable in one place.
/// Tera receives this via `Context::from_serialize`.
#[derive(Debug, Clone, Serialize)]
struct TemplateView<'a> {
    invoice: &'a Invoice,
    customer: &'a Customer,
    lines: &'a [InvoiceLine],
    /// Convenience: invoice number or "Draft".
    invoice_label: String,
    /// Convenience: lowercase currency code (e.g. `"cad"`).
    currency_code: String,
    /// Convenience: minor-unit scale for the currency (decimals).
    currency_scale: u32,
}

impl RenderContext {
    /// Construct from the three primary inputs.
    pub fn new(invoice: Invoice, customer: Customer, lines: Vec<InvoiceLine>) -> Self {
        Self {
            invoice,
            customer,
            lines,
        }
    }

    /// Build the Tera [`Context`] this render uses.
    ///
    /// [`Context`]: tera::Context
    pub fn to_tera_context(&self) -> TeraContext {
        let view = TemplateView {
            invoice: &self.invoice,
            customer: &self.customer,
            lines: &self.lines,
            invoice_label: self
                .invoice
                .number
                .clone()
                .unwrap_or_else(|| "Draft".to_string()),
            currency_code: self.invoice.currency.as_str().to_lowercase(),
            currency_scale: self.invoice.currency.scale(),
        };
        TeraContext::from_serialize(&view)
            .expect("RenderContext serializes cleanly into Tera context")
    }
}
