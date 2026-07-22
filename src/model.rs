use crate::error::{PrometheusError, Result, escape_control_characters};
use serde::{Deserialize, Deserializer, de, de::DeserializeOwned};
use std::{collections::BTreeMap, fmt, sync::Arc};

const MICROS_PER_SECOND: f64 = 1_000_000.0;
// `i64::MAX as f64` rounds up to 2^63, so this is an exclusive upper bound.
const MIN_MICROS: f64 = -9_223_372_036_854_775_808.0;
const MAX_MICROS_EXCLUSIVE: f64 = 9_223_372_036_854_775_808.0;

pub type LabelSet = Arc<[(String, String)]>;

#[derive(Debug)]
pub struct SampleRow {
    pub labels: LabelSet,
    pub timestamp_micros: i64,
    pub value: f64,
}

#[derive(Debug)]
pub struct MetadataRow {
    pub metric: String,
    pub metric_type: String,
    pub help: String,
    pub unit: String,
}

#[derive(Debug, Deserialize)]
struct ResponseEnvelope<T> {
    status: String,
    data: Option<T>,
    #[serde(rename = "errorType")]
    error_type: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QueryRangeData {
    #[serde(rename = "resultType")]
    result_type: String,
    result: Vec<MatrixSeries>,
}

#[derive(Debug, Deserialize)]
struct InstantQueryData {
    #[serde(rename = "resultType")]
    result_type: String,
    result: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct MatrixSeries {
    metric: BTreeMap<String, String>,
    #[serde(default)]
    values: Vec<FloatSample>,
    #[serde(default)]
    histograms: Vec<serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
struct VectorSeries {
    metric: BTreeMap<String, String>,
    value: Option<FloatSample>,
    histogram: Option<serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
struct FloatSample(
    f64,
    #[serde(deserialize_with = "deserialize_sample_value")] f64,
);

#[derive(Debug, Deserialize)]
struct MetadataEntry {
    #[serde(rename = "type")]
    metric_type: String,
    help: String,
    #[serde(default)]
    unit: String,
}

fn deserialize_sample_value<'de, D>(deserializer: D) -> std::result::Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    struct SampleValueVisitor;

    impl de::Visitor<'_> for SampleValueVisitor {
        type Value = f64;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a Prometheus sample value string")
        }

        fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            parse_sample_value(v).map_err(E::custom)
        }
    }

    deserializer.deserialize_str(SampleValueVisitor)
}

pub fn parse_range_response(body: &[u8]) -> Result<Vec<SampleRow>> {
    let data: QueryRangeData = parse_envelope(body)?;
    if data.result_type != "matrix" {
        return Err(PrometheusError::UnexpectedRangeResultType(data.result_type));
    }
    flatten_matrix(data.result)
}

pub fn parse_instant_response(body: &[u8]) -> Result<Vec<SampleRow>> {
    let data: InstantQueryData = parse_envelope(body)?;
    match data.result_type.as_str() {
        "matrix" => {
            let series =
                serde_json::from_value(data.result).map_err(PrometheusError::InvalidResponse)?;
            flatten_matrix(series)
        }
        "vector" => {
            let series: Vec<VectorSeries> =
                serde_json::from_value(data.result).map_err(PrometheusError::InvalidResponse)?;
            flatten_vector(series)
        }
        "scalar" => {
            let FloatSample(timestamp, value) =
                serde_json::from_value(data.result).map_err(PrometheusError::InvalidResponse)?;
            Ok(vec![SampleRow {
                labels: Arc::from([]),
                timestamp_micros: seconds_to_micros(timestamp)?,
                value,
            }])
        }
        _ => Err(PrometheusError::UnsupportedInstantResultType(
            data.result_type,
        )),
    }
}

pub fn parse_string_response(body: &[u8]) -> Result<Vec<String>> {
    parse_envelope(body)
}

pub fn parse_series_response(body: &[u8]) -> Result<Vec<LabelSet>> {
    let series: Vec<BTreeMap<String, String>> = parse_envelope(body)?;
    Ok(series
        .into_iter()
        .map(|labels| labels.into_iter().collect())
        .collect())
}

pub fn parse_metadata_response(body: &[u8]) -> Result<Vec<MetadataRow>> {
    let metadata: BTreeMap<String, Vec<MetadataEntry>> = parse_envelope(body)?;
    let row_count = metadata
        .values()
        .try_fold(0_usize, |count, entries| count.checked_add(entries.len()));
    let mut rows = Vec::with_capacity(row_count.ok_or(PrometheusError::MetadataCountOverflow)?);

    for (metric, entries) in metadata {
        for entry in entries {
            rows.push(MetadataRow {
                metric: metric.clone(),
                metric_type: entry.metric_type,
                help: entry.help,
                unit: entry.unit,
            });
        }
    }
    Ok(rows)
}

fn parse_envelope<T>(body: &[u8]) -> Result<T>
where
    T: DeserializeOwned,
{
    let envelope: ResponseEnvelope<T> =
        serde_json::from_slice(body).map_err(PrometheusError::InvalidResponse)?;

    if envelope.status != "success" {
        return Err(PrometheusError::Api {
            error_type: envelope.error_type.map_or_else(
                || "unknown".to_string(),
                |value| escape_control_characters(&value),
            ),
            message: envelope.error.map_or_else(
                || "no error message".to_string(),
                |value| escape_control_characters(&value),
            ),
        });
    }

    envelope.data.ok_or(PrometheusError::MissingData)
}

fn flatten_matrix(series: Vec<MatrixSeries>) -> Result<Vec<SampleRow>> {
    if series.iter().any(|item| !item.histograms.is_empty()) {
        return Err(PrometheusError::UnsupportedNativeHistograms);
    }

    let row_count = series
        .iter()
        .try_fold(0_usize, |count, item| count.checked_add(item.values.len()));
    let mut rows = Vec::with_capacity(row_count.ok_or(PrometheusError::SampleCountOverflow)?);

    for item in series {
        let labels: LabelSet = item.metric.into_iter().collect();
        for FloatSample(timestamp, value) in item.values {
            rows.push(SampleRow {
                labels: Arc::clone(&labels),
                timestamp_micros: seconds_to_micros(timestamp)?,
                value,
            });
        }
    }
    Ok(rows)
}

fn flatten_vector(series: Vec<VectorSeries>) -> Result<Vec<SampleRow>> {
    if series.iter().any(|item| item.histogram.is_some()) {
        return Err(PrometheusError::UnsupportedNativeHistograms);
    }

    let mut rows = Vec::with_capacity(series.len());
    for item in series {
        let FloatSample(timestamp, value) = item
            .value
            .ok_or(PrometheusError::MissingInstantFloatSample)?;
        rows.push(SampleRow {
            labels: item.metric.into_iter().collect(),
            timestamp_micros: seconds_to_micros(timestamp)?,
            value,
        });
    }
    Ok(rows)
}

fn seconds_to_micros(seconds: f64) -> Result<i64> {
    if !seconds.is_finite() {
        return Err(PrometheusError::NonFiniteTimestamp(seconds));
    }

    let micros = (seconds * MICROS_PER_SECOND).round();
    if !micros.is_finite() || !(MIN_MICROS..MAX_MICROS_EXCLUSIVE).contains(&micros) {
        return Err(PrometheusError::TimestampOutOfRange(seconds));
    }

    // The finite range check above proves this rounded value fits in i64.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the preceding finite range check proves the rounded value fits in i64"
    )]
    Ok(micros as i64)
}

fn parse_sample_value(value: &str) -> Result<f64> {
    match value {
        "NaN" => Ok(f64::NAN),
        "+Inf" => Ok(f64::INFINITY),
        "-Inf" => Ok(f64::NEG_INFINITY),
        _ => {
            let parsed =
                value
                    .parse::<f64>()
                    .map_err(|source| PrometheusError::InvalidSampleValue {
                        value: value.to_string(),
                        source,
                    })?;
            if parsed.is_finite() {
                Ok(parsed)
            } else {
                Err(PrometheusError::NonFiniteSampleValue(value.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_range(body: &str) -> Result<Vec<SampleRow>> {
        parse_range_response(body.as_bytes())
    }

    #[test]
    fn parses_matrix_and_sorts_labels() {
        let rows = parse_range(
            r#"{
                "status":"success",
                "data":{
                    "resultType":"matrix",
                    "result":[{
                        "metric":{"job":"prometheus","__name__":"up"},
                        "values":[[1721469600.125,"1"],[1721469660,"2.5"]]
                    }]
                }
            }"#,
        )
        .expect("valid matrix response");

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].labels.as_ref(),
            [
                ("__name__".to_string(), "up".to_string()),
                ("job".to_string(), "prometheus".to_string()),
            ]
        );
        assert!(Arc::ptr_eq(&rows[0].labels, &rows[1].labels));
        assert_eq!(rows[0].timestamp_micros, 1_721_469_600_125_000);
        assert!((rows[0].value - 1.0).abs() < f64::EPSILON);
        assert!((rows[1].value - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_instant_vector_scalar_and_matrix_results() {
        let vector = parse_instant_response(
            br#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{"job":"api"},"value":[1,"2"]}]}}"#,
        )
        .expect("valid vector");
        assert_eq!(vector.len(), 1);
        assert_eq!(vector[0].labels[0], ("job".to_string(), "api".to_string()));
        assert!((vector[0].value - 2.0).abs() < f64::EPSILON);

        let scalar = parse_instant_response(
            br#"{"status":"success","data":{"resultType":"scalar","result":[2,"3"]}}"#,
        )
        .expect("valid scalar");
        assert!(scalar[0].labels.is_empty());
        assert!((scalar[0].value - 3.0).abs() < f64::EPSILON);

        let matrix = parse_instant_response(
            br#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{},"values":[[3,"4"]]}]}}"#,
        )
        .expect("valid instant matrix");
        assert!((matrix[0].value - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_discovery_responses() {
        let values = parse_string_response(br#"{"status":"success","data":["a","b"]}"#)
            .expect("valid string list");
        assert_eq!(values, ["a", "b"]);

        let series = parse_series_response(
            br#"{"status":"success","data":[{"job":"api","__name__":"up"}]}"#,
        )
        .expect("valid series list");
        assert_eq!(series[0][0].0, "__name__");

        let metadata = parse_metadata_response(
            br#"{"status":"success","data":{"up":[{"type":"gauge","help":"Up","unit":""}]}}"#,
        )
        .expect("valid metadata");
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].metric, "up");
        assert_eq!(metadata[0].metric_type, "gauge");
    }

    #[test]
    fn parses_empty_result() {
        let rows =
            parse_range(r#"{"status":"success","data":{"resultType":"matrix","result":[]}}"#)
                .expect("valid empty response");
        assert!(rows.is_empty());
    }

    #[test]
    fn accepts_large_sample_response() {
        const LARGE_SAMPLE_COUNT: usize = 1_000_001;

        let mut body = String::with_capacity(LARGE_SAMPLE_COUNT * 8);
        body.push_str(
            r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{},"values":["#,
        );
        for index in 0..LARGE_SAMPLE_COUNT {
            if index != 0 {
                body.push(',');
            }
            body.push_str(r#"[1,"1"]"#);
        }
        body.push_str("]}]}}");

        let rows = parse_range(&body).expect("sample count is governed by the Prometheus server");
        assert_eq!(rows.len(), LARGE_SAMPLE_COUNT);
    }

    #[test]
    fn reports_api_error_envelope() {
        let error =
            parse_range(r#"{"status":"error","errorType":"bad_data","error":"invalid query"}"#)
                .expect_err("error response must fail");
        assert!(error.to_string().contains("bad_data"));
        assert!(error.to_string().contains("invalid query"));
    }

    #[test]
    fn escapes_control_characters_in_api_diagnostics() {
        let api_error = parse_range(
            r#"{"status":"error","errorType":"bad\u0000data","error":"first\nsecond"}"#,
        )
        .expect_err("error response must fail");
        assert_eq!(
            api_error.to_string(),
            r"Prometheus API returned an error (bad\u{0}data): first\nsecond"
        );
    }

    #[test]
    fn accepts_successful_data_with_warnings() {
        let rows = parse_range(
            r#"{
                "status":"success",
                "warnings":["store gateway unavailable"],
                "data":{
                    "resultType":"matrix",
                    "result":[{"metric":{},"values":[[1,"2"]]}]
                }
            }"#,
        )
        .expect("warnings do not invalidate successful data");
        assert_eq!(rows.len(), 1);
        assert!((rows[0].value - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rejects_unsupported_instant_strings_and_native_histograms() {
        let string_error = parse_instant_response(
            br#"{"status":"success","data":{"resultType":"string","result":[1,"ready"]}}"#,
        )
        .expect_err("string result must fail");
        assert!(string_error.to_string().contains("string"));

        for body in [
            br#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"histogram":[1,{}]}]}}"#.as_slice(),
            br#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{},"histograms":[[1,{}]]}]}}"#.as_slice(),
        ] {
            assert!(matches!(
                parse_instant_response(body),
                Err(PrometheusError::UnsupportedNativeHistograms)
            ));
        }
    }

    #[test]
    fn rejects_non_matrix_range_results() {
        let error =
            parse_range(r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#)
                .expect_err("vector range response must fail");
        assert!(error.to_string().contains("expected \"matrix\""));
    }

    #[test]
    fn parses_prometheus_special_values() {
        assert!(
            parse_sample_value("NaN")
                .expect("NaN is supported")
                .is_nan()
        );
        let positive = parse_sample_value("+Inf").expect("positive infinity is supported");
        assert!(positive.is_infinite() && positive.is_sign_positive());
        let negative = parse_sample_value("-Inf").expect("negative infinity is supported");
        assert!(negative.is_infinite() && negative.is_sign_negative());
        parse_sample_value("Infinity").expect_err("unsupported infinity spelling must fail");
    }

    #[test]
    fn rejects_invalid_sample_value_in_response() {
        let error = parse_range(
            r#"{
                "status":"success",
                "data":{
                    "resultType":"matrix",
                    "result":[{"metric":{},"values":[[1,"not-a-number"]]}]
                }
            }"#,
        )
        .expect_err("an invalid sample value must fail");
        assert!(
            error
                .to_string()
                .contains("invalid Prometheus sample value")
        );
    }

    #[test]
    fn validates_timestamps() {
        assert_eq!(
            seconds_to_micros(-0.25).expect("negative timestamps are supported"),
            -250_000
        );
        seconds_to_micros(f64::NAN).expect_err("NaN timestamp must fail");
        seconds_to_micros(f64::INFINITY).expect_err("infinite timestamp must fail");
        seconds_to_micros(10_000_000_000_000.0)
            .expect_err("timestamp above the supported range must fail");
        seconds_to_micros(-10_000_000_000_000.0)
            .expect_err("timestamp below the supported range must fail");
    }
}
