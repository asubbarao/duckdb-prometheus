.PHONY: check clean clean_all configure format_check lint test_rust test_environment

PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

EXTENSION_NAME=prometheus

# Set to 1 to enable Unstable API (binaries will only work on TARGET_DUCKDB_VERSION, forwards compatibility will be broken)
# Note: currently extension-template-rs requires this, as duckdb-rs relies on unstable C API functionality
USE_UNSTABLE_C_API=1

# Target DuckDB version
TARGET_DUCKDB_VERSION=v1.5.4
DUCKDB_TEST_VERSION?=$(patsubst v%,%,$(TARGET_DUCKDB_VERSION))

RUST_ENV=DUCKDB_EXTENSION_NAME=$(EXTENSION_NAME) DUCKDB_EXTENSION_MIN_DUCKDB_VERSION=$(TARGET_DUCKDB_VERSION)
UV?=uv
UV_ENVIRONMENT_STAMP=configure/venv/.prometheus-test-environment

all: configure debug

# Include makefiles from DuckDB
include extension-ci-tools/makefiles/c_api_extensions/base.Makefile
include extension-ci-tools/makefiles/c_api_extensions/rust.Makefile

configure: test_environment
	$(MAKE) platform extension_version

debug: build_extension_library_debug build_extension_with_metadata_debug
release: build_extension_library_release build_extension_with_metadata_release

test: test_debug
test_debug: test_extension_debug test_python_debug
test_release: test_extension_release test_python_release

clean: clean_build clean_rust
clean_all: clean_configure clean

ifneq ($(shell command -v $(UV) 2>/dev/null),)
test_environment: $(UV_ENVIRONMENT_STAMP)
else
# The DuckDB distribution runners do not guarantee uv is installed, so retain
# compatibility with the vendored bootstrap there. Local development uses uv.
test_environment: venv
endif

$(UV_ENVIRONMENT_STAMP): Makefile test/requirements.txt
	$(UV) venv --allow-existing --python $(PYTHON_BIN) configure/venv
	$(UV) pip sync --python $(PYTHON_VENV_BIN) test/requirements.txt
	$(PYTHON_VENV_BIN) -c "import duckdb, duckdb_sqllogictest, packaging"
	$(PYTHON_VENV_BIN) -c "from pathlib import Path;Path('$@').touch()"

format_check:
	cargo fmt --all -- --check

lint: format_check
	$(RUST_ENV) cargo clippy --locked --all-targets --all-features -- -D warnings

test_rust:
	$(RUST_ENV) cargo test --locked --all-targets --all-features

check: lint test_rust

.PHONY: test_python_debug test_python_release

ifeq ($(SKIP_TESTS),1)
test_python_debug test_python_release: tests_skipped
else
test_python_debug: check_configure
	$(PYTHON_VENV_BIN) test/python/test_prometheus_integration.py build/debug/prometheus.duckdb_extension

test_python_release: check_configure
	$(PYTHON_VENV_BIN) test/python/test_prometheus_integration.py build/release/prometheus.duckdb_extension
endif
