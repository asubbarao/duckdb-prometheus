mod bind;
mod client;
mod error;
mod model;
mod scan;

use duckdb::{Connection, Result, duckdb_entrypoint_c_api};
use scan::{
    PrometheusQueryManyVTab, PrometheusQueryVTab, PrometheusScanManyVTab, PrometheusScanVTab,
};
use std::error::Error;

#[duckdb_entrypoint_c_api]
/// Registers the extension's functions with a `DuckDB` connection.
///
/// # Errors
///
/// Returns an error if `DuckDB` cannot register the table function.
///
/// # Safety
///
/// `DuckDB` must call this entrypoint with a valid connection supplied through
/// its loadable-extension C API.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the DuckDB entrypoint macro requires an owned Connection"
)]
pub unsafe fn extension_entrypoint(con: Connection) -> Result<(), Box<dyn Error>> {
    con.register_table_function::<PrometheusScanVTab>("prometheus_scan")?;
    con.register_table_function::<PrometheusScanManyVTab>("prometheus_scan_many")?;
    con.register_table_function::<PrometheusQueryVTab>("prometheus_query")?;
    con.register_table_function::<PrometheusQueryManyVTab>("prometheus_query_many")?;
    Ok(())
}
