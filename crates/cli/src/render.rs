//! Render helpers wrapping `hop_top_kit::output::dispatch`.
//!
//! Every CLI command — including single-row outputs like a freshly
//! drafted invoice or a tick summary — funnels through `dispatch` so the
//! kit's `--format / --cols / --output / --template / --format-help`
//! flag suite stays uniformly available. Helpers below cover the two
//! common shapes:
//!
//! - [`render_value`] for a single object (`show`, command results)
//! - [`render_list`] for an array of rows (`list`, tick summaries)

use anyhow::{anyhow, Result};
use clap::ArgMatches;
use hop_top_kit::output::{dispatch, ColumnSpec, DispatchOptions};
use serde_json::Value;

/// Render a single JSON value as a one-row table / object / etc.
///
/// `table` is a degenerate single-row, multi-column shape; `json` /
/// `yaml` serialize the object verbatim. The `columns` schema is
/// consulted only for `--cols` validation + the `table` projection.
pub fn render_value(matches: &ArgMatches, value: Value, columns: &[ColumnSpec]) -> Result<()> {
    // The table formatter renders arrays of rows. For single-value
    // outputs we wrap in a one-element array so `table` stays useful
    // (one row of all the columns). `json` / `yaml` see the same array;
    // operators after a single object can pipe through `jq '.[0]'` or
    // `--cols` + `--format yaml`.
    let payload = Value::Array(vec![value]);
    dispatch_inner(matches, &payload, columns)
}

/// Render a JSON array of rows.
pub fn render_list(matches: &ArgMatches, rows: Value, columns: &[ColumnSpec]) -> Result<()> {
    dispatch_inner(matches, &rows, columns)
}

fn dispatch_inner(matches: &ArgMatches, data: &Value, columns: &[ColumnSpec]) -> Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    dispatch(
        matches,
        &mut lock,
        data,
        DispatchOptions {
            columns: if columns.is_empty() {
                None
            } else {
                Some(columns)
            },
            ..Default::default()
        },
    )
    .map_err(|e| anyhow!("output dispatch: {e}"))
}
