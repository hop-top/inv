//! Render pipeline: HTML (via tera) + PDF (engine behind a Cargo feature).
//!
//! ## Contract
//!
//! Two entry points:
//!
//! - [`html::render_html`] — takes an optional filesystem template path
//!   and a [`context::RenderContext`], returns rendered HTML. If no path
//!   is supplied, the bundled `invoice.html.tera` is used.
//! - [`pdf::render_pdf`] — takes the HTML from the previous step and
//!   returns PDF bytes. Implementation is behind one of the
//!   `pdf-*` Cargo features (see crate-level docs).
//!
//! ## PDF engine state at v1
//!
//! The default feature is `pdf-stub`, which returns the HTML bytes
//! verbatim. That keeps the surface wired end-to-end (commands → blob
//! store → channels) without committing to a heavy PDF dep yet. The
//! "real" engines (`pdf-typst`, `pdf-wkhtmltopdf`, `pdf-weasyprint`)
//! are scaffolded but TODO at v1.1.
//!
//! See design spec §11 (render config) and §12 (PDF engine open Q).

pub mod context;
pub mod html;
pub mod pdf;
pub mod template;

pub use context::RenderContext;
pub use html::{render_html, RenderError};
pub use pdf::{render_pdf, PdfError};
pub use template::TemplateSource;
