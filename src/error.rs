use reqwest::StatusCode;
use std::{io, num::ParseFloatError};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, PrometheusError>;

pub(crate) fn escape_control_characters(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            escaped.extend(character.escape_default());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[derive(Debug, Error)]
pub enum PrometheusError {
    #[error("invalid Prometheus endpoint URL: {0}")]
    InvalidEndpoint(#[source] url::ParseError),

    #[error("Prometheus endpoint URL must use http or https")]
    UnsupportedEndpointScheme,

    #[error("Prometheus endpoint URL must be an absolute HTTP URL")]
    EndpointIsNotAbsolute,

    #[error("Prometheus endpoint URL must not contain credentials")]
    EndpointContainsCredentials,

    #[error("Prometheus endpoint URL must not contain a fragment")]
    EndpointContainsFragment,

    #[error("failed to build Prometheus HTTP client: {0}")]
    BuildClient(#[source] reqwest::Error),

    #[error("Prometheus HTTP request failed: {0}")]
    Request(#[source] reqwest::Error),

    #[error("failed to read Prometheus HTTP response: {0}")]
    ReadResponse(#[source] io::Error),

    #[error("failed to read Prometheus HTTP error response ({status}): {source}")]
    ReadErrorResponse {
        status: StatusCode,
        #[source]
        source: io::Error,
    },

    #[error("Prometheus HTTP request returned {status}; response body: {preview}")]
    HttpStatus { status: StatusCode, preview: String },

    #[error("invalid Prometheus API response: {0}")]
    InvalidResponse(#[source] serde_json::Error),

    #[error("Prometheus API returned an error ({error_type}): {message}")]
    Api { error_type: String, message: String },

    #[error("Prometheus success response is missing data")]
    MissingData,

    #[error("Prometheus query_range returned resultType {0:?}; expected \"matrix\"")]
    UnexpectedRangeResultType(String),

    #[error("Prometheus instant query returned unsupported resultType {0:?}")]
    UnsupportedInstantResultType(String),

    #[error("Prometheus query_range returned native histogram samples, which are not supported")]
    UnsupportedNativeHistograms,

    #[error("Prometheus response contains too many samples")]
    SampleCountOverflow,

    #[error("Prometheus response contains too many labels in one output chunk")]
    LabelCountOverflow,

    #[error("Prometheus metadata response contains too many entries")]
    MetadataCountOverflow,

    #[error("Prometheus instant vector result is missing a float sample")]
    MissingInstantFloatSample,

    #[error("invalid non-finite Prometheus timestamp: {0}")]
    NonFiniteTimestamp(f64),

    #[error("Prometheus timestamp is out of range: {0}")]
    TimestampOutOfRange(f64),

    #[error("invalid Prometheus sample value {value:?}: {source}")]
    InvalidSampleValue {
        value: String,
        #[source]
        source: ParseFloatError,
    },

    #[error("invalid Prometheus sample value: {0:?}")]
    NonFiniteSampleValue(String),

    #[error("{0}")]
    InvalidArgument(String),

    #[error("Prometheus table-function bind data is unavailable")]
    MissingBindData,

    #[error("invalid projected column index: {0}")]
    InvalidProjectedColumn(u64),

    #[error("Prometheus table-function projection metadata mismatch")]
    ProjectionMismatch,

    #[error("Prometheus table-function state lock is poisoned")]
    StatePoisoned,

    #[error("DuckDB returned an invalid vector capacity")]
    InvalidVectorCapacity,
}
