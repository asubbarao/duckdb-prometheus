use crate::{
    bind::{
        client_config, connection_named_parameters, optional_named_string,
        optional_named_timestamp, required_parameter,
    },
    client::{
        ClientConfig, DiscoveryConfig, fetch_label_values, fetch_labels, fetch_metadata,
        fetch_series,
    },
    error::{PrometheusError, Result as ExtensionResult},
    model::{LabelSet, MetadataRow},
};
use duckdb::{
    core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId},
    vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab},
};
use std::{error::Error, sync::Mutex};

type VTabResult<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug)]
enum StringRequest {
    Labels(DiscoveryConfig),
    LabelValues {
        config: DiscoveryConfig,
        label: String,
    },
}

#[derive(Debug)]
pub struct StringBindData {
    request: StringRequest,
}

#[derive(Debug)]
struct RowsState<T> {
    rows: Vec<T>,
    offset: usize,
}

#[derive(Debug)]
pub struct StringInitData {
    state: Mutex<RowsState<String>>,
    projected: bool,
}

#[derive(Debug)]
pub struct SeriesBindData {
    config: DiscoveryConfig,
}

#[derive(Debug)]
pub struct SeriesInitData {
    state: Mutex<RowsState<LabelSet>>,
    projected: bool,
}

#[derive(Debug)]
pub struct MetadataBindData {
    client: ClientConfig,
    metric: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum MetadataColumn {
    Metric,
    Type,
    Help,
    Unit,
}

#[derive(Debug)]
pub struct MetadataInitData {
    state: Mutex<RowsState<MetadataRow>>,
    projected_columns: Vec<MetadataColumn>,
}

pub struct PrometheusLabelsVTab;
pub struct PrometheusLabelValuesVTab;
pub struct PrometheusSeriesVTab;
pub struct PrometheusMetadataVTab;

impl VTab for PrometheusLabelsVTab {
    type InitData = StringInitData;
    type BindData = StringBindData;

    fn bind(bind: &BindInfo) -> VTabResult<Self::BindData> {
        let config = discovery_config(bind, optional_named_string(bind, "matcher").map_err(boxed)?)
            .map_err(boxed)?;
        bind.add_result_column("label", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        Ok(StringBindData {
            request: StringRequest::Labels(config),
        })
    }

    fn init(init: &InitInfo) -> VTabResult<Self::InitData> {
        init_strings(init)
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> VTabResult<()> {
        emit_strings(func.get_init_data(), output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(discovery_named_parameters(true))
    }
}

impl VTab for PrometheusLabelValuesVTab {
    type InitData = StringInitData;
    type BindData = StringBindData;

    fn bind(bind: &BindInfo) -> VTabResult<Self::BindData> {
        let label = required_parameter(bind, 0, "label").map_err(boxed)?;
        let config = discovery_config(bind, optional_named_string(bind, "matcher").map_err(boxed)?)
            .map_err(boxed)?;
        bind.add_result_column("value", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        Ok(StringBindData {
            request: StringRequest::LabelValues { config, label },
        })
    }

    fn init(init: &InitInfo) -> VTabResult<Self::InitData> {
        init_strings(init)
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> VTabResult<()> {
        emit_strings(func.get_init_data(), output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(discovery_named_parameters(true))
    }
}

impl VTab for PrometheusSeriesVTab {
    type InitData = SeriesInitData;
    type BindData = SeriesBindData;

    fn bind(bind: &BindInfo) -> VTabResult<Self::BindData> {
        let matcher = required_parameter(bind, 0, "matcher").map_err(boxed)?;
        let config = discovery_config(bind, Some(matcher)).map_err(boxed)?;
        let varchar = LogicalTypeHandle::from(LogicalTypeId::Varchar);
        bind.add_result_column("labels", LogicalTypeHandle::map(&varchar, &varchar));
        Ok(SeriesBindData { config })
    }

    fn init(init: &InitInfo) -> VTabResult<Self::InitData> {
        init.set_max_threads(1);
        let bind_data = bind_data::<SeriesBindData>(init).map_err(boxed)?;
        let projected = single_projection(init).map_err(boxed)?;
        let rows = fetch_series(&bind_data.config).map_err(boxed)?;
        Ok(SeriesInitData {
            state: Mutex::new(RowsState { rows, offset: 0 }),
            projected,
        })
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> VTabResult<()> {
        emit_series(func.get_init_data(), output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(discovery_named_parameters(false))
    }
}

impl VTab for PrometheusMetadataVTab {
    type InitData = MetadataInitData;
    type BindData = MetadataBindData;

    fn bind(bind: &BindInfo) -> VTabResult<Self::BindData> {
        let client = client_config(bind).map_err(boxed)?;
        let metric = optional_named_string(bind, "metric").map_err(boxed)?;
        for column in ["metric", "type", "help", "unit"] {
            bind.add_result_column(column, LogicalTypeHandle::from(LogicalTypeId::Varchar));
        }
        Ok(MetadataBindData { client, metric })
    }

    fn init(init: &InitInfo) -> VTabResult<Self::InitData> {
        init.set_max_threads(1);
        let bind_data = bind_data::<MetadataBindData>(init).map_err(boxed)?;
        let projected_columns = init
            .get_column_indices()
            .into_iter()
            .map(metadata_column)
            .collect::<ExtensionResult<Vec<_>>>()
            .map_err(boxed)?;
        let rows = fetch_metadata(&bind_data.client, bind_data.metric.as_deref()).map_err(boxed)?;
        Ok(MetadataInitData {
            state: Mutex::new(RowsState { rows, offset: 0 }),
            projected_columns,
        })
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> VTabResult<()> {
        emit_metadata(func.get_init_data(), output)
    }

    fn supports_pushdown() -> bool {
        true
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        let mut parameters = connection_named_parameters();
        parameters.push((
            "metric".to_string(),
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        ));
        Some(parameters)
    }
}

fn discovery_named_parameters(include_matcher: bool) -> Vec<(String, LogicalTypeHandle)> {
    let mut parameters = connection_named_parameters();
    if include_matcher {
        parameters.push((
            "matcher".to_string(),
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        ));
    }
    parameters.extend([
        (
            "start_time".to_string(),
            LogicalTypeHandle::from(LogicalTypeId::Any),
        ),
        (
            "end_time".to_string(),
            LogicalTypeHandle::from(LogicalTypeId::Any),
        ),
    ]);
    parameters
}

fn discovery_config(bind: &BindInfo, matcher: Option<String>) -> ExtensionResult<DiscoveryConfig> {
    DiscoveryConfig::new(
        client_config(bind)?,
        matcher,
        optional_named_timestamp(bind, "start_time")?,
        optional_named_timestamp(bind, "end_time")?,
    )
}

fn init_strings(init: &InitInfo) -> VTabResult<StringInitData> {
    init.set_max_threads(1);
    let bind_data = bind_data::<StringBindData>(init).map_err(boxed)?;
    let projected = single_projection(init).map_err(boxed)?;
    let rows = match &bind_data.request {
        StringRequest::Labels(config) => fetch_labels(config),
        StringRequest::LabelValues { config, label } => fetch_label_values(config, label),
    }
    .map_err(boxed)?;
    Ok(StringInitData {
        state: Mutex::new(RowsState { rows, offset: 0 }),
        projected,
    })
}

fn emit_strings(init_data: &StringInitData, output: &mut DataChunkHandle) -> VTabResult<()> {
    let mut state = init_data
        .state
        .lock()
        .map_err(|_| boxed(PrometheusError::StatePoisoned))?;
    let Some((end, rows)) = next_rows(&state, vector_capacity().map_err(boxed)?) else {
        output.set_len(0);
        return Ok(());
    };
    validate_output_columns(output, usize::from(init_data.projected)).map_err(boxed)?;
    if init_data.projected {
        let vector = output.flat_vector(0);
        for (index, value) in rows.iter().enumerate() {
            vector.insert(index, value);
        }
    }
    let output_len = rows.len();
    state.offset = end;
    drop(state);
    output.set_len(output_len);
    Ok(())
}

fn emit_series(init_data: &SeriesInitData, output: &mut DataChunkHandle) -> VTabResult<()> {
    let mut state = init_data
        .state
        .lock()
        .map_err(|_| boxed(PrometheusError::StatePoisoned))?;
    let Some((end, rows)) = next_rows(&state, vector_capacity().map_err(boxed)?) else {
        output.set_len(0);
        return Ok(());
    };
    validate_output_columns(output, usize::from(init_data.projected)).map_err(boxed)?;
    if init_data.projected {
        insert_label_sets(output, 0, rows).map_err(boxed)?;
    }
    let output_len = rows.len();
    state.offset = end;
    drop(state);
    output.set_len(output_len);
    Ok(())
}

fn emit_metadata(init_data: &MetadataInitData, output: &mut DataChunkHandle) -> VTabResult<()> {
    let mut state = init_data
        .state
        .lock()
        .map_err(|_| boxed(PrometheusError::StatePoisoned))?;
    let Some((end, rows)) = next_rows(&state, vector_capacity().map_err(boxed)?) else {
        output.set_len(0);
        return Ok(());
    };
    validate_output_columns(output, init_data.projected_columns.len()).map_err(boxed)?;
    for (output_index, column) in init_data.projected_columns.iter().enumerate() {
        let vector = output.flat_vector(output_index);
        for (row_index, row) in rows.iter().enumerate() {
            let value = match column {
                MetadataColumn::Metric => &row.metric,
                MetadataColumn::Type => &row.metric_type,
                MetadataColumn::Help => &row.help,
                MetadataColumn::Unit => &row.unit,
            };
            vector.insert(row_index, value);
        }
    }
    let output_len = rows.len();
    state.offset = end;
    drop(state);
    output.set_len(output_len);
    Ok(())
}

fn next_rows<T>(state: &RowsState<T>, capacity: usize) -> Option<(usize, &[T])> {
    if state.offset >= state.rows.len() {
        return None;
    }
    let end = state.rows.len().min(state.offset.saturating_add(capacity));
    Some((end, &state.rows[state.offset..end]))
}

fn insert_label_sets(
    output: &DataChunkHandle,
    output_index: usize,
    rows: &[LabelSet],
) -> ExtensionResult<()> {
    let label_count = rows
        .iter()
        .try_fold(0_usize, |count, labels| count.checked_add(labels.len()));
    let label_count = label_count.ok_or(PrometheusError::LabelCountOverflow)?;

    let mut map = output.list_vector(output_index);
    let entries = map.struct_child(label_count);
    let keys = entries.child(0, label_count);
    let values = entries.child(1, label_count);
    let mut offset = 0_usize;
    for (row_index, labels) in rows.iter().enumerate() {
        map.set_entry(row_index, offset, labels.len());
        for (key, value) in labels.iter() {
            keys.insert(offset, key);
            values.insert(offset, value);
            offset += 1;
        }
    }
    map.set_len(label_count);
    Ok(())
}

fn bind_data<T>(init: &InitInfo) -> ExtensionResult<&T> {
    let bind_data = init.get_bind_data::<T>();
    // SAFETY: duckdb-rs stores the VTab BindData in a Box during bind and
    // keeps it alive for the complete init callback. Each caller requests the
    // exact BindData type associated with that callback.
    unsafe { bind_data.as_ref() }.ok_or(PrometheusError::MissingBindData)
}

fn single_projection(init: &InitInfo) -> ExtensionResult<bool> {
    let indices = init.get_column_indices();
    if indices.iter().all(|index| *index == 0) && indices.len() <= 1 {
        Ok(!indices.is_empty())
    } else {
        Err(PrometheusError::InvalidProjectedColumn(
            indices.into_iter().find(|index| *index != 0).unwrap_or(1),
        ))
    }
}

fn metadata_column(index: u64) -> ExtensionResult<MetadataColumn> {
    match index {
        0 => Ok(MetadataColumn::Metric),
        1 => Ok(MetadataColumn::Type),
        2 => Ok(MetadataColumn::Help),
        3 => Ok(MetadataColumn::Unit),
        _ => Err(PrometheusError::InvalidProjectedColumn(index)),
    }
}

fn validate_output_columns(output: &DataChunkHandle, expected: usize) -> ExtensionResult<()> {
    if output.num_columns() == expected {
        Ok(())
    } else {
        Err(PrometheusError::ProjectionMismatch)
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
