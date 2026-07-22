# DuckDB Prometheus extension

[![CI](https://github.com/botan/duckdb-prometheus/actions/workflows/MainDistributionPipeline.yml/badge.svg)](https://github.com/botan/duckdb-prometheus/actions/workflows/MainDistributionPipeline.yml)
[![License](https://img.shields.io/github/license/botan/duckdb-prometheus)](LICENSE)

Query Prometheus-compatible HTTP APIs, including Thanos Query, directly from
DuckDB. Use PromQL to select samples, then use SQL to inspect, aggregate, join,
or export the results.

> [!WARNING]
> This is an experimental, Codex-generated project. Use it at your own risk.
> The API is unstable and may change without notice.

## Quick start

The extension builds from source against DuckDB v1.5.4. You need
Rust 1.86 or newer, Python 3, [uv](https://docs.astral.sh/uv/), Make, and Git.

```shell
git clone --recurse-submodules https://github.com/botan/duckdb-prometheus.git
cd duckdb-prometheus
make configure
make debug
```

Start DuckDB with support for unsigned local extensions, then load the build:

```shell
duckdb -unsigned
```

```sql
LOAD './build/debug/prometheus.duckdb_extension';
```

Query the last 15 minutes of the standard `up` metric, replacing the endpoint
with your Prometheus or Thanos Query URL:

```sql
SELECT timestamp, job, instance, value
FROM prometheus_scan(
    'up',
    current_timestamp - INTERVAL '15 minutes',
    current_timestamp,
    endpoint := 'http://localhost:9090',
    step := INTERVAL '1 minute',
    labels := ['job', 'instance']
)
ORDER BY timestamp DESC, job, instance;
```

Each sample becomes one row. The complete label set remains in the `labels`
map; the `labels` argument also exposes selected labels as nullable `VARCHAR`
columns.

To load the local build from Python:

```python
import duckdb

con = duckdb.connect(config={"allow_unsigned_extensions": "true"})
con.execute("LOAD './build/debug/prometheus.duckdb_extension'")
```

## Examples

### Combine PromQL and SQL

Filter and aggregate in Prometheus when possible to reduce the data transferred
to DuckDB. This query calculates a per-job request rate in PromQL, then produces
hourly averages in SQL:

```sql
SELECT
    date_trunc('hour', timestamp) AS hour,
    job,
    avg(value) AS requests_per_second
FROM prometheus_scan(
    'sum by (job) (rate(http_requests_total[5m]))',
    current_timestamp - INTERVAL '24 hours',
    current_timestamp,
    endpoint := 'http://localhost:9090',
    step := INTERVAL '1 minute',
    labels := ['job']
)
GROUP BY ALL
ORDER BY ALL;
```

### Run an instant query

`prometheus_query` evaluates PromQL once at the Prometheus server's current
time:

```sql
SELECT job, instance, value
FROM prometheus_query(
    'up',
    endpoint := 'http://localhost:9090',
    labels := ['job', 'instance']
)
ORDER BY job, instance;
```

Set `evaluation_time := TIMESTAMPTZ '2026-07-20 10:00:00+00'` to evaluate at a
specific time.

### Discover metrics

List metric names known to the server:

```sql
SELECT value AS metric
FROM prometheus_label_values(
    '__name__',
    endpoint := 'http://localhost:9090'
)
ORDER BY metric;
```

## Query functions

| Function | Purpose |
|---|---|
| `prometheus_scan(query, start, end, ...)` | Run one range query |
| `prometheus_scan_many(queries, start, end, ...)` | Run several range queries |
| `prometheus_query(query, ...)` | Run one instant query |
| `prometheus_query_many(queries, ...)` | Run several instant queries |

`query` values are raw PromQL. Range bounds accept `TIMESTAMPTZ`, `TIMESTAMP`,
or timestamp strings. Values without an offset are interpreted as UTC, and
range bounds are inclusive.

| Named argument | Functions | Type | Default |
|---|---|---|---|
| `endpoint` | All | `VARCHAR` | Required |
| `request_timeout` | All | `INTERVAL` | No total deadline |
| `labels` | All | `VARCHAR[]` | `[]` |
| `step` | Range | `INTERVAL` | `1 minute` |
| `evaluation_time` | Instant | Timestamp or timestamp string | Server time |

The endpoint may include a reverse-proxy path and existing query parameters.
Requests are currently unauthenticated. `request_timeout` and `step` must be
positive fixed intervals; months and years are not accepted.

All four query functions return the same base columns:

| Column | Type | Description |
|---|---|---|
| `labels` | `MAP(VARCHAR, VARCHAR)` | Series labels, sorted by key |
| `timestamp` | `TIMESTAMP WITH TIME ZONE` | Absolute sample time |
| `value` | `DOUBLE` | Sample value, including `NaN` and infinities |

Read a label with `labels['job']`, or promote frequently used labels to columns
with `labels := ['job', 'instance']`. Promoted names must be unique and cannot
be `labels`, `timestamp`, or `value`.

Instant vectors, scalars, and matrices are flattened to this schema. The plural
functions require a non-empty list, execute requests one at a time, and combine
the results with `UNION ALL` semantics. If one request fails, the complete query
fails; `request_timeout` applies separately to each request.

## Discovery functions

All discovery functions require `endpoint` and also accept `request_timeout`.

| Function | Result columns |
|---|---|
| `prometheus_labels(...)` | `label VARCHAR` |
| `prometheus_label_values(label, ...)` | `value VARCHAR` |
| `prometheus_series(matcher, ...)` | `labels MAP(VARCHAR, VARCHAR)` |
| `prometheus_metadata(...)` | `metric VARCHAR`, `type VARCHAR`, `help VARCHAR`, `unit VARCHAR` |

`prometheus_labels` and `prometheus_label_values` optionally accept `matcher`.
Those two functions and `prometheus_series` also accept `start_time` and
`end_time`. `prometheus_metadata` optionally accepts `metric`.

## Build and test

For an optimized build, run `make release` and load
`build/release/prometheus.duckdb_extension`.

```shell
make check
make debug
make test_debug
make release
make test_release
```

`make check` runs formatting, Clippy with warnings denied, and Rust tests. The
integration tests use an ephemeral local Prometheus-compatible server.

## Current limitations

- DuckDB filters and aggregations are not pushed into PromQL.
- Responses have no extension-imposed byte, series, or sample limit. Singular
  functions buffer one response; plural functions buffer one response at a
  time. Large responses can exhaust process memory.
- Native histogram samples and instant-query string results are not supported.
- Prometheus warnings are not exposed.
- Authentication, custom headers, retries, time-range chunking, paging, and
  caching are not implemented.
- Discovery functions accept one matcher, not repeated `match[]` values.
- The blocking HTTP client does not support WASM.

## License

Apache License 2.0. See [LICENSE](LICENSE).
