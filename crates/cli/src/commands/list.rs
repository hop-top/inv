use clap::ArgMatches;
use hop_top_kit::output::{dispatch, ColumnSpec, DispatchOptions};
use serde_json::json;

/// Demo list command — exercises the kit output flag suite.
///
/// Try:
///
///   inv list
///   inv list --format json
///   inv list --format yaml
///   inv list --cols name,status
///   inv list -o /tmp/out.json   # ext infers json
///   inv list --format-help
pub fn run(matches: &ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    let items = json!([
        {"name": "alpha", "count": 1, "status": "ok"},
        {"name": "beta",  "count": 2, "status": "warn"},
    ]);
    let schema = [
        ColumnSpec::new("name", "name", 9),
        ColumnSpec::new("count", "count", 7),
        ColumnSpec::new("status", "status", 5),
    ];
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    dispatch(
        matches,
        &mut lock,
        &items,
        DispatchOptions {
            columns: Some(&schema),
            ..Default::default()
        },
    )?;
    Ok(())
}
