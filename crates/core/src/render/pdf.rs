//! PDF rendering — engine selected by Cargo feature.
//!
//! v1 ships [`render_pdf`] behind a feature gate. The default feature
//! is `pdf-stub`, which returns the HTML bytes verbatim. The "real"
//! engines (`pdf-typst`, `pdf-wkhtmltopdf`, `pdf-weasyprint`) are
//! scaffolded but not implemented; turning them on at v1 yields a
//! deliberate [`PdfError::EngineNotImplemented`].
//!
//! See design spec §11 (config) and §12 (open Q on engine choice).

use thiserror::Error;

/// Errors from [`render_pdf`].
#[derive(Debug, Error)]
pub enum PdfError {
    /// No PDF feature was enabled at build time.
    #[error(
        "no PDF engine compiled in; rebuild inv-core with one of \
         `pdf-stub`, `pdf-typst`, `pdf-wkhtmltopdf`, `pdf-weasyprint`"
    )]
    NoEngine,
    /// The selected engine is scaffolded but not yet implemented.
    #[error("PDF engine `{0}` is selected but not implemented at v1")]
    EngineNotImplemented(&'static str),
    /// The engine ran but failed (subprocess non-zero exit, etc.).
    ///
    /// Reserved for the real engines at v1.1.
    #[error("PDF engine failed: {0}")]
    Engine(String),
}

/// Render HTML to a PDF byte buffer using the engine compiled in.
///
/// `pdf-stub` (the default) returns the HTML bytes verbatim, which is
/// enough to let callers exercise the contract (write a blob, hand it
/// to a channel, etc.) without committing to a real engine.
pub async fn render_pdf(html: &str) -> Result<Vec<u8>, PdfError> {
    // Engine selection is mutually exclusive in intent; we pick the
    // first one that's compiled in. `pdf-stub` is the default so a
    // bare `cargo build` yields a working pipeline.
    #[cfg(feature = "pdf-stub")]
    {
        return Ok(html.as_bytes().to_vec());
    }

    #[cfg(all(feature = "pdf-typst", not(feature = "pdf-stub")))]
    {
        let _ = html;
        return Err(PdfError::EngineNotImplemented("pdf-typst"));
    }

    #[cfg(all(
        feature = "pdf-wkhtmltopdf",
        not(feature = "pdf-stub"),
        not(feature = "pdf-typst"),
    ))]
    {
        let _ = html;
        return Err(PdfError::EngineNotImplemented("pdf-wkhtmltopdf"));
    }

    #[cfg(all(
        feature = "pdf-weasyprint",
        not(feature = "pdf-stub"),
        not(feature = "pdf-typst"),
        not(feature = "pdf-wkhtmltopdf"),
    ))]
    {
        let _ = html;
        return Err(PdfError::EngineNotImplemented("pdf-weasyprint"));
    }

    #[cfg(not(any(
        feature = "pdf-stub",
        feature = "pdf-typst",
        feature = "pdf-wkhtmltopdf",
        feature = "pdf-weasyprint",
    )))]
    {
        let _ = html;
        return Err(PdfError::NoEngine);
    }
}
