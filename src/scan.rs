use crate::{
    bind::{
        client_config, connection_named_parameters, invalid_argument, named_interval,
        named_string_list, optional_named_timestamp, required_parameter,
        required_string_list_parameter, required_timestamp,
    },
    client::{InstantQueryConfig, RangeQueryConfig, fetch_instant, fetch_range},
    error::{PrometheusError, Result as ExtensionResult},
    model::SampleRow,
};
use duckdb::{
    core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId},
    ffi::duckdb_timestamp,
    vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab},
};
use std::{
    collections::{BTreeSet, VecDeque},
    error::Error,
    sync::Mutex,
    time::Duration,
};

const DEFAULT_STEP: Duration = Duration::from_secs(60);

type VTabResult<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug, Clone)]
enum SampleRequest {
    Range(RangeQueryConfig),
    Instant(InstantQueryConfig),
}

#[derive(Debug)]
pub struct SampleBindData {
    requests: Vec<SampleRequest>,
    promoted_labels: Vec<String>,
}

#[derive(Debug)]
struct ScanState {
    pending_requests: VecDeque<SampleRequest>,
    rows: Vec<SampleRow>,
    offset: usize,
}

#[derive(Debug, Clone)]
enum OutputColumn {
    Labels,
    Timestamp,
    Value,
    PromotedLabel(String),
}

#[derive(Debug)]
pub struct SampleInitData {
    state: Mutex<ScanState>,
    projected_columns: Vec<OutputColumn>,
}

pub struct PrometheusScanVTab;
pub struct PrometheusScanManyVTab;
pub struct PrometheusQueryVTab;
pub struct PrometheusQueryManyVTab;

impl VTab for PrometheusScanVTab {
    type InitData = SampleInitData;
    type BindData = SampleBindData;

    fn bind(bind: &BindInfo) -> VTabResult<Self::BindData> {
        let query = required_parameter(bind, 0, "query").map_err(boxed)?;
        bind_range_queries(bind, vec![query])
    }

    fn init(init: &InitInfo) -> VTabResult<Self::InitData> {
        init_samples(init)
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> VTabResult<()> {
        emit_samples(func.get_init_data(), output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        // ANY preserves the bound timestamp type for strict validation.
        Some(vec![
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
            LogicalTypeHandle::from(LogicalTypeId::Any),
            LogicalTypeHandle::from(LogicalTypeId::Any),
        ])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(range_named_parameters())
    }
}

impl VTab for PrometheusScanManyVTab {
    type InitData = SampleInitData;
    type BindData = SampleBindData;

    fn bind(bind: &BindInfo) -> VTabResult<Self::BindData> {
        let queries = required_string_list_parameter(bind, 0, "queries").map_err(boxed)?;
        bind_range_queries(bind, queries)
    }

    fn init(init: &InitInfo) -> VTabResult<Self::InitData> {
        init_samples(init)
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> VTabResult<()> {
        emit_samples(func.get_init_data(), output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        // ANY preserves the bound timestamp type for strict validation.
        Some(vec![
            LogicalTypeHandle::list(&LogicalTypeHandle::from(LogicalTypeId::Varchar)),
            LogicalTypeHandle::from(LogicalTypeId::Any),
            LogicalTypeHandle::from(LogicalTypeId::Any),
        ])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(range_named_parameters())
    }
}

impl VTab for PrometheusQueryVTab {
    type InitData = SampleInitData;
    type BindData = SampleBindData;

    fn bind(bind: &BindInfo) -> VTabResult<Self::BindData> {
        let query = required_parameter(bind, 0, "query").map_err(boxed)?;
        bind_instant_queries(bind, vec![query])
    }

    fn init(init: &InitInfo) -> VTabResult<Self::InitData> {
        init_samples(init)
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> VTabResult<()> {
        emit_samples(func.get_init_data(), output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(instant_named_parameters())
    }
}

impl VTab for PrometheusQueryManyVTab {
    type InitData = SampleInitData;
    type BindData = SampleBindData;

    fn bind(bind: &BindInfo) -> VTabResult<Self::BindData> {
        let queries = required_string_list_parameter(bind, 0, "queries").map_err(boxed)?;
        bind_instant_queries(bind, queries)
    }

    fn init(init: &InitInfo) -> VTabResult<Self::InitData> {
        init_samples(init)
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> VTabResult<()> {
        emit_samples(func.get_init_data(), output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::list(&LogicalTypeHandle::from(
            LogicalTypeId::Varchar,
        ))])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(instant_named_parameters())
    }
}

fn bind_range_queries(bind: &BindInfo, queries: Vec<String>) -> VTabResult<SampleBindData> {
    let client = client_config(bind).map_err(boxed)?;
    let start = required_timestamp(bind, 1, "start").map_err(boxed)?;
    let end = required_timestamp(bind, 2, "end").map_err(boxed)?;
    let step = named_interval(bind, "step")
        .map_err(boxed)?
        .unwrap_or(DEFAULT_STEP);
    let promoted_labels = promoted_labels(bind).map_err(boxed)?;
    bind_sample_columns(bind, &promoted_labels);

    let requests = queries
        .into_iter()
        .map(|query| {
            RangeQueryConfig::new(client.clone(), query, start, end, step).map(SampleRequest::Range)
        })
        .collect::<ExtensionResult<Vec<_>>>()
        .map_err(boxed)?;
    Ok(SampleBindData {
        requests,
        promoted_labels,
    })
}

fn bind_instant_queries(bind: &BindInfo, queries: Vec<String>) -> VTabResult<SampleBindData> {
    let client = client_config(bind).map_err(boxed)?;
    let time = optional_named_timestamp(bind, "evaluation_time").map_err(boxed)?;
    let promoted_labels = promoted_labels(bind).map_err(boxed)?;
    bind_sample_columns(bind, &promoted_labels);

    let requests = queries
        .into_iter()
        .map(|query| SampleRequest::Instant(InstantQueryConfig::new(client.clone(), query, time)))
        .collect();
    Ok(SampleBindData {
        requests,
        promoted_labels,
    })
}

fn range_named_parameters() -> Vec<(String, LogicalTypeHandle)> {
    let mut parameters = connection_named_parameters();
    parameters.extend([
        (
            "step".to_string(),
            LogicalTypeHandle::from(LogicalTypeId::Interval),
        ),
        (
            "labels".to_string(),
            LogicalTypeHandle::list(&LogicalTypeHandle::from(LogicalTypeId::Varchar)),
        ),
    ]);
    parameters
}

fn instant_named_parameters() -> Vec<(String, LogicalTypeHandle)> {
    let mut parameters = connection_named_parameters();
    parameters.extend([
        (
            "evaluation_time".to_string(),
            LogicalTypeHandle::from(LogicalTypeId::Any),
        ),
        (
            "labels".to_string(),
            LogicalTypeHandle::list(&LogicalTypeHandle::from(LogicalTypeId::Varchar)),
        ),
    ]);
    parameters
}

fn bind_sample_columns(bind: &BindInfo, promoted_labels: &[String]) {
    let varchar = LogicalTypeHandle::from(LogicalTypeId::Varchar);
    bind.add_result_column("labels", LogicalTypeHandle::map(&varchar, &varchar));
    bind.add_result_column(
        "timestamp",
        LogicalTypeHandle::from(LogicalTypeId::TimestampTZ),
    );
    bind.add_result_column("value", LogicalTypeHandle::from(LogicalTypeId::Double));
    for label in promoted_labels {
        bind.add_result_column(label, LogicalTypeHandle::from(LogicalTypeId::Varchar));
    }
}

fn promoted_labels(bind: &BindInfo) -> ExtensionResult<Vec<String>> {
    let labels = named_string_list(bind, "labels")?;
    let reserved = ["labels", "timestamp", "value"];
    let mut names = BTreeSet::new();

    for label in &labels {
        if label.contains('\0') {
            return Err(invalid_argument("labels must not contain NUL characters"));
        }
        let folded = label.to_lowercase();
        if reserved.contains(&folded.as_str()) {
            return Err(invalid_argument(format!(
                "promoted label {label:?} conflicts with an existing result column"
            )));
        }
        if !names.insert(folded) {
            return Err(invalid_argument(format!(
                "promoted label {label:?} is duplicated"
            )));
        }
    }
    Ok(labels)
}

fn init_samples(init: &InitInfo) -> VTabResult<SampleInitData> {
    init.set_max_threads(1);
    let bind_data = bind_data(init).map_err(boxed)?;
    let projected_columns = init
        .get_column_indices()
        .into_iter()
        .map(|index| output_column(index, &bind_data.promoted_labels))
        .collect::<ExtensionResult<Vec<_>>>()
        .map_err(boxed)?;
    let mut pending_requests = VecDeque::from(bind_data.requests.clone());
    let rows = fetch_next_rows(&mut pending_requests).map_err(boxed)?;

    Ok(SampleInitData {
        state: Mutex::new(ScanState {
            pending_requests,
            rows,
            offset: 0,
        }),
        projected_columns,
    })
}

fn fetch_next_rows(requests: &mut VecDeque<SampleRequest>) -> ExtensionResult<Vec<SampleRow>> {
    while let Some(request) = requests.pop_front() {
        let rows = match &request {
            SampleRequest::Range(config) => fetch_range(config),
            SampleRequest::Instant(config) => fetch_instant(config),
        }?;
        if !rows.is_empty() {
            return Ok(rows);
        }
    }
    Ok(Vec::new())
}

fn bind_data(init: &InitInfo) -> ExtensionResult<&SampleBindData> {
    let bind_data = init.get_bind_data::<SampleBindData>();
    // SAFETY: duckdb-rs stores `SampleBindData` in a Box during bind and
    // keeps it alive for the complete init callback. The requested type is
    // the exact VTab BindData associated with both sample callbacks.
    unsafe { bind_data.as_ref() }.ok_or(PrometheusError::MissingBindData)
}

fn output_column(index: u64, promoted_labels: &[String]) -> ExtensionResult<OutputColumn> {
    match index {
        0 => Ok(OutputColumn::Labels),
        1 => Ok(OutputColumn::Timestamp),
        2 => Ok(OutputColumn::Value),
        _ => {
            let label_index = usize::try_from(index - 3)
                .map_err(|_| PrometheusError::InvalidProjectedColumn(index))?;
            promoted_labels
                .get(label_index)
                .cloned()
                .map(OutputColumn::PromotedLabel)
                .ok_or(PrometheusError::InvalidProjectedColumn(index))
        }
    }
}

fn emit_samples(init_data: &SampleInitData, output: &mut DataChunkHandle) -> VTabResult<()> {
    let mut state = init_data
        .state
        .lock()
        .map_err(|_| boxed(PrometheusError::StatePoisoned))?;
    if state.offset >= state.rows.len() {
        state.rows = Vec::new();
        state.offset = 0;
        let rows = fetch_next_rows(&mut state.pending_requests).map_err(boxed)?;
        state.rows = rows;
        if state.rows.is_empty() {
            output.set_len(0);
            return Ok(());
        }
    }
    if output.num_columns() != init_data.projected_columns.len() {
        return Err(boxed(PrometheusError::ProjectionMismatch));
    }

    let capacity = vector_capacity().map_err(boxed)?;
    let end = state.rows.len().min(state.offset.saturating_add(capacity));
    let rows = &state.rows[state.offset..end];
    let output_len = rows.len();

    for (output_index, column) in init_data.projected_columns.iter().enumerate() {
        match column {
            OutputColumn::Labels => insert_labels(output, output_index, rows).map_err(boxed)?,
            OutputColumn::Timestamp => insert_timestamps(output, output_index, rows),
            OutputColumn::Value => insert_values(output, output_index, rows),
            OutputColumn::PromotedLabel(label) => {
                insert_promoted_label(output, output_index, rows, label);
            }
        }
    }

    state.offset = end;
    drop(state);
    output.set_len(output_len);
    Ok(())
}

fn insert_labels(
    output: &DataChunkHandle,
    output_index: usize,
    rows: &[SampleRow],
) -> ExtensionResult<()> {
    let label_count = rows
        .iter()
        .try_fold(0_usize, |count, row| count.checked_add(row.labels.len()));
    let label_count = label_count.ok_or(PrometheusError::LabelCountOverflow)?;

    let mut map = output.list_vector(output_index);
    let entries = map.struct_child(label_count);
    let keys = entries.child(0, label_count);
    let values = entries.child(1, label_count);
    let mut offset = 0_usize;

    for (row_index, row) in rows.iter().enumerate() {
        map.set_entry(row_index, offset, row.labels.len());
        for (key, value) in row.labels.iter() {
            keys.insert(offset, key);
            values.insert(offset, value);
            offset += 1;
        }
    }
    map.set_len(label_count);
    Ok(())
}

fn insert_timestamps(output: &DataChunkHandle, output_index: usize, rows: &[SampleRow]) {
    let timestamps: Vec<duckdb_timestamp> = rows
        .iter()
        .map(|row| duckdb_timestamp {
            micros: row.timestamp_micros,
        })
        .collect();
    let mut vector = output.flat_vector(output_index);
    // SAFETY: bind declared this output as TIMESTAMP_TZ, whose physical
    // representation is `duckdb_timestamp`. The slice is bounded by the
    // DuckDB vector capacity and this is the only writable wrapper.
    unsafe { vector.copy(&timestamps) };
}

fn insert_values(output: &DataChunkHandle, output_index: usize, rows: &[SampleRow]) {
    let values: Vec<f64> = rows.iter().map(|row| row.value).collect();
    let mut vector = output.flat_vector(output_index);
    // SAFETY: bind declared this output as DOUBLE, whose physical
    // representation is `f64`. The slice is bounded by the DuckDB vector
    // capacity and this is the only writable wrapper.
    unsafe { vector.copy(&values) };
}

fn insert_promoted_label(
    output: &DataChunkHandle,
    output_index: usize,
    rows: &[SampleRow],
    label: &str,
) {
    let mut vector = output.flat_vector(output_index);
    for (row_index, row) in rows.iter().enumerate() {
        match row
            .labels
            .binary_search_by(|(key, _)| key.as_str().cmp(label))
        {
            Ok(index) => vector.insert(row_index, &row.labels[index].1),
            Err(_) => vector.set_null(row_index),
        }
    }
}

fn vector_capacity() -> ExtensionResult<usize> {
    // SAFETY: `duckdb_vector_size` is a process-global C API query with no
    // pointer arguments or caller-maintained invariants.
    let capacity = unsafe { duckdb::ffi::duckdb_vector_size() };
    let capacity = usize::try_from(capacity).map_err(|_| PrometheusError::InvalidVectorCapacity)?;
    if capacity == 0 {
        Err(PrometheusError::InvalidVectorCapacity)
    } else {
        Ok(capacity)
    }
}

fn boxed(error: PrometheusError) -> Box<dyn Error> {
    Box::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn resolves_dynamic_output_columns() {
        let labels = vec!["job".to_string(), "instance".to_string()];
        assert!(matches!(
            output_column(3, &labels).expect("job column"),
            OutputColumn::PromotedLabel(label) if label == "job"
        ));
        assert!(matches!(
            output_column(4, &labels).expect("instance column"),
            OutputColumn::PromotedLabel(label) if label == "instance"
        ));
        output_column(5, &labels).expect_err("out-of-range column must fail");
    }

    #[test]
    fn label_sets_are_sorted_for_binary_lookup() {
        let row = SampleRow {
            labels: Arc::from([
                ("__name__".to_string(), "up".to_string()),
                ("job".to_string(), "api".to_string()),
            ]),
            timestamp_micros: 0,
            value: 1.0,
        };
        let index = row
            .labels
            .binary_search_by(|(key, _)| key.as_str().cmp("job"))
            .expect("job exists");
        assert_eq!(row.labels[index].1, "api");
    }
}
