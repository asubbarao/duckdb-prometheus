## 1. Understand before acting

- Inspect relevant code, tests, and configuration before changing anything.
- State material assumptions; proceed on low-risk, reversible ones.
- Ask only when ambiguity materially affects behaviour, compatibility, security, public interfaces, or scope.
- Surface meaningful tradeoffs and prefer the simplest viable option.

## 2. Make the smallest correct change

- Every changed line must support the requested outcome.
- Avoid speculative features, abstractions, and configurability.
- Match existing structure and style; do not refactor, reformat, or remove unrelated code.
- Remove only unused code created by your change.
- Do not guard states ruled out by clear internal invariants. Validate external input, protocol responses, conversions, and FFI boundaries.

## 3. Preserve contracts and boundaries

- Treat public signatures, parameter rules, result schemas, and user-visible errors as compatibility contracts; update documentation and tests when they change.
- Preserve supported toolchain and runtime versions unless explicitly asked to change them.
- Keep validation, networking, response parsing, data emission, and registration in their existing layers.
- Keep unsafe code narrow and document the invariant that makes it valid.
- Do not change network policy, including retries, caching, concurrency, or authentication, unless requested.

## 4. Verify proportionately

- Define expected behaviour before coding. Reproduce bugs first; establish a passing baseline before refactors.
- Unit tests for parsing, conversion, and isolated logic.
- SQL logic tests for binding, schemas, validation, and stable errors.
- Hermetic integration tests for requests, responses, chunking, and end-to-end behaviour.
- Run `make check` for Rust changes.
- Run `make debug` and `make test_debug` for externally visible behaviour.
- Add release and minimum-toolchain checks for build, dependency, compatibility, or distribution changes.
- Report which checks ran and which did not.

## 5. Protect the workspace

- Treat submodules and vendored infrastructure as read-only unless explicitly in scope.
- Prefer repository Make targets because they provide required build metadata.
- Change dependencies or lockfiles only when required; do not commit generated build artefacts.
- Preserve unrelated user changes.
