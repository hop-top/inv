//! Template source + Tera loader.
//!
//! Two source modes:
//!
//! - [`TemplateSource::Bundled`] — the default template compiled into
//!   the binary via [`include_str!`]. Always available.
//! - [`TemplateSource::Filesystem`] — a path to a `.tera` template on
//!   disk. Read at render time; no on-disk caching beyond Tera's own.
//!
//! v1 keeps things simple: each call builds a fresh [`tera::Tera`]
//! instance scoped to one template. The per-request build cost is in
//! the microsecond range for a sub-300-line template; if profiling
//! later shows it on the hot path we can swap in an `OnceCell` cache
//! keyed by path + mtime.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Name Tera uses for the bundled template (only matters in error msgs).
pub const BUNDLED_TEMPLATE_NAME: &str = "invoice.html.tera";

/// Raw source of the bundled default invoice template.
pub const BUNDLED_INVOICE_TEMPLATE: &str =
    include_str!("../../../../templates/default/invoice.html.tera");

/// Where the renderer reads its template from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSource {
    /// Use the template baked into the binary.
    Bundled,
    /// Read a `.tera` template from this filesystem path.
    Filesystem(PathBuf),
}

impl TemplateSource {
    /// Resolve an optional path to a source: `Some(p)` → filesystem;
    /// `None` → bundled.
    pub fn resolve(path: Option<&Path>) -> Self {
        match path {
            Some(p) => TemplateSource::Filesystem(p.to_path_buf()),
            None => TemplateSource::Bundled,
        }
    }
}

/// Errors loading or compiling a Tera template.
#[derive(Debug, Error)]
pub enum TemplateError {
    /// Reading the on-disk template file failed.
    #[error("could not read template file `{path}`: {source}")]
    Read {
        /// The path we tried to read.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// Tera rejected the template source (syntax error, unclosed tag, etc.).
    #[error("template compile failed: {0}")]
    Compile(#[from] tera::Error),
}

/// Build a [`tera::Tera`] instance containing exactly one template,
/// named so error messages stay readable.
pub fn build_tera(source: &TemplateSource) -> Result<tera::Tera, TemplateError> {
    let mut tera = tera::Tera::default();
    // Autoescape HTML for `.html.tera` extensions (Tera's default rule).
    tera.autoescape_on(vec![".html.tera", ".html"]);

    let (name, body): (String, String) = match source {
        TemplateSource::Bundled => (
            BUNDLED_TEMPLATE_NAME.to_string(),
            BUNDLED_INVOICE_TEMPLATE.to_string(),
        ),
        TemplateSource::Filesystem(path) => {
            let body = std::fs::read_to_string(path).map_err(|source| TemplateError::Read {
                path: path.clone(),
                source,
            })?;
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| BUNDLED_TEMPLATE_NAME.to_string());
            (name, body)
        }
    };

    tera.add_raw_template(&name, &body)?;
    Ok(tera)
}

/// Lookup name (matches what [`build_tera`] registered the template as).
pub fn template_name(source: &TemplateSource) -> String {
    match source {
        TemplateSource::Bundled => BUNDLED_TEMPLATE_NAME.to_string(),
        TemplateSource::Filesystem(path) => path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| BUNDLED_TEMPLATE_NAME.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_template_compiles() {
        let tera = build_tera(&TemplateSource::Bundled).unwrap();
        assert!(tera
            .get_template_names()
            .any(|n| n == BUNDLED_TEMPLATE_NAME));
    }

    #[test]
    fn resolve_none_is_bundled() {
        assert_eq!(TemplateSource::resolve(None), TemplateSource::Bundled);
    }

    #[test]
    fn resolve_some_is_filesystem() {
        let p = Path::new("/tmp/x.tera");
        assert_eq!(
            TemplateSource::resolve(Some(p)),
            TemplateSource::Filesystem(p.to_path_buf())
        );
    }
}
