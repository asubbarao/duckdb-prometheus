# DuckDB Prometheus extension

Query Prometheus-compatible HTTP APIs, including Thanos Query, directly from
DuckDB.

```sql
FROM prometheus_scan(
    'node_cpu_seconds_total{mode="idle"}',
    '2026-07-20 10:00:00',
    '2026-07-20 11:00:00',
    endpoint := 'http://localhost:9090',
    step := INTERVAL '1 minute',
    labels := ['job', 'instance']
);
```

## Design rationale

This extension treats Prometheus as a query service rather than exposing every
metric as a SQL table. Callers provide PromQL and explicit evaluation bounds,
and the extension returns the resulting samples as DuckDB rows.

This approach:

- does not enumerate all metric names before binding a known query;
- preserves PromQL label matching, functions, and aggregation semantics;
- keeps filtering and aggregation in Prometheus or Thanos when expressed in
  PromQL, reducing unnecessary data transfer;
- avoids translating SQL table scans into generated range-vector queries;
- supports instant queries, range queries, and discovery through explicit
  table functions.

## Connection

Every function requires `endpoint := VARCHAR` and optionally accepts
`request_timeout := INTERVAL`. Endpoints may include reverse-proxy paths and
existing query parameters. Requests are unauthenticated. The timeout must be a
positive fixed interval; when omitted, the client has no total deadline.

## Query functions

```text
prometheus_scan(query, start, end, endpoint := VARCHAR, ⟨options⟩)
prometheus_scan_many(queries, start, end, endpoint := VARCHAR, ⟨options⟩)
prometheus_query(query, endpoint := VARCHAR, ⟨options⟩)
prometheus_query_many(queries, endpoint := VARCHAR, ⟨options⟩)
```

| Option | Functions | Type | Default |
|---|---|---|---|
| `step` | Range | `INTERVAL` | `1 minute` |
| `evaluation_time` | Instant | Timestamp | Server time |
| `request_timeout` | All | `INTERVAL` | Unlimited |
| `labels` | All | `VARCHAR[]` | `[]` |

`query` values are raw PromQL. Range bounds accept `TIMESTAMPTZ`, `TIMESTAMP`,
or timestamp strings; values without an offset are UTC. Bounds are inclusive.

All four functions return:

| Column | Type | Description |
|---|---|---|
| `labels` | `MAP(VARCHAR, VARCHAR)` | Deterministically key-sorted series labels |
| `timestamp` | `TIMESTAMP WITH TIME ZONE` | Absolute sample instant |
| `value` | `DOUBLE` | Float sample value, including `NaN` and infinities |

Use `labels['job']` to read the map, or `labels := ['job', 'instance']` to add
nullable `VARCHAR` columns while retaining it. Promoted names must be unique
and cannot be `labels`, `timestamp`, or `value`.

Instant float vectors, scalars, and matrices are flattened to the same schema.
Plural functions require a non-empty query list, execute one request at a time,
and return `UNION ALL` semantics, including duplicates. A failed request fails
the complete query; `request_timeout` applies to each request.

## Discovery

| Function | Result columns |
|---|---|
| `prometheus_labels()` | `label VARCHAR` |
| `prometheus_label_values(label)` | `value VARCHAR` |
| `prometheus_series(matcher)` | `labels MAP(VARCHAR, VARCHAR)` |
| `prometheus_metadata()` | `metric VARCHAR`, `type VARCHAR`, `help VARCHAR`, `unit VARCHAR` |

All require `endpoint`. Labels and label values accept `matcher`; labels, label
values, and series accept `start_time` and `end_time`; metadata accepts
`metric`. All are optional named arguments except the positional values shown
above.

## Build and load

Prerequisites are Rust 1.86 or newer, Python 3, uv, Make, and Git. Clone with
submodules, then build against DuckDB v1.5.4:

```shell
git clone --recurse-submodules https://github.com/botan/duckdb-prometheus.git
cd duckdb-prometheus
make configure
make debug
```

Load the unsigned local extension with the DuckDB CLI or Python client:

```sql
LOAD './build/debug/prometheus.duckdb_extension';
```

Use DuckDB's `-unsigned` CLI option when loading a local unsigned extension.
For an optimized build, run `make release` and load the artifact under
`build/release`.

## Verify

```shell
make check
make debug
make test_debug
make release
make test_release
```

`make check` runs formatting, Clippy with warnings denied, and Rust tests. The
integration tests use an ephemeral local server.

## Current limitations

- DuckDB filters and aggregations are not pushed into PromQL.
- Successful responses have no extension-imposed byte, series, or sample
  limit. Singular functions buffer one response; plural functions buffer one
  response at a time. Large responses can exhaust process memory.
- Native histogram samples and instant-query string results are not supported.
- Successful data is returned when Prometheus includes warnings; warnings are
  not exposed.
- Authentication, custom headers, retries, time-range chunking, paging, and
  caching are not implemented.
- Discovery helpers accept one matcher, not repeated `match[]` values.
- The blocking HTTP client does not support WASM.

## License

Apache License 2.0. See [LICENSE](LICENSE).
