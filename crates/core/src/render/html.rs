//! HTML rendering via Tera.

use super::context::RenderContext;
use super::template::{build_tera, template_name, TemplateError, TemplateSource};
use std::path::Path;
use thiserror::Error;

/// Errors from [`render_html`].
#[derive(Debug, Error)]
pub enum RenderError {
    /// Loading or compiling the Tera template failed.
    #[error(transparent)]
    Template(#[from] TemplateError),
    /// Tera rejected the template render (missing variable, etc.).
    #[error("template render failed: {0}")]
    Render(#[from] tera::Error),
}

/// Render an invoice to HTML.
///
/// `template_path = None` uses the bundled default template. Any
/// supplied path is read from disk on every call (no caching at v1).
///
/// Async only for forward compatibility with engines that want to do IO
/// off the request thread (e.g. weasyprint subprocess); the v1 body is
/// pure CPU.
pub async fn render_html(
    template_path: Option<&Path>,
    ctx: &RenderContext,
) -> Result<String, RenderError> {
    let source = TemplateSource::resolve(template_path);
    let tera = build_tera(&source)?;
    let name = template_name(&source);
    let tera_ctx = ctx.to_tera_context();
    Ok(tera.render(&name, &tera_ctx)?)
}
