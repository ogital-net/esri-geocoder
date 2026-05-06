# arcgis-geocoder

Async Rust client library for the ArcGIS World Geocoding Service.

## Project overview

This crate wraps ArcGIS geocoding REST endpoints with typed request/response models and an async client built on `reqwest`.

## Build, lint, and test commands

```sh
cargo build
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets -- -W clippy::pedantic
```

Feature backends are mutually exclusive. To lint non-default JSON backends, run one feature at a time:

```sh
cargo clippy --all-targets --no-default-features --features sonic-rs -- -W clippy::pedantic
cargo clippy --all-targets --no-default-features --features simd-json -- -W clippy::pedantic
```

## Cargo features

```toml
[features]
default = ["serde_json"]
serde_json = ["dep:serde_json"]
sonic-rs   = ["dep:sonic-rs"]
simd-json  = ["dep:simd-json"]
```

Only one JSON backend feature should be active at once. `src/lib.rs` contains compile-time guards enforcing this.

## Cargo dependencies

```toml
[dependencies]
reqwest      = { version = "0.13", default-features = false, features = ["rustls", "http2", "query", "form"] }
serde        = { version = "1", features = ["derive"] }
serde_json   = { version = "1", optional = true }
sonic-rs     = { version = "0", optional = true }
simd-json    = { version = "0", optional = true }
thiserror    = "2"
log          = "0"
tokio        = { version = "1", default-features = false, features = ["sync"] }

[dev-dependencies]
serde_json  = "1"
wiremock    = "0.6"
tokio       = { version = "1", features = ["macros", "rt-multi-thread"] }
dotenvy     = "0.15"
env_logger  = "0.11"
```

## Source layout conventions

Use this top-to-bottom layout in `src/lib.rs`:

1. `#![doc = include_str!("../README.md")]`
2. `compile_error!` feature guards for mutually exclusive JSON backends
3. `mod json` backend shim with `#[cfg(feature = "...")]` blocks
4. Public backend aliases: `pub use json::Value as JsonValue;` and `pub use json::Error as JsonError;`
5. imports
6. `pub mod models;` and `pub use models::*;`
7. `pub type Result<T> = std::result::Result<T, Error>;`
8. crate `Error` enum with `Http`, `Api { code, message }`, and `Json` variants
9. internal ArcGIS error-envelope structs (`ArcGisErrorBody`, `ErrorEnvelope`)
10. logging helpers (`log_request`, `log_response_head`)
11. shared response parser (`check_and_deserialize`)
12. `RequestBuilderExt` trait containing `.send_json::<T>()`
13. `ClientBuilder`
14. `GeocoderClient` plus endpoint methods
15. `#[cfg(test)] mod tests`

`src/models.rs` contains all public request/response model types.

## HTTP and request conventions

- `GeocoderClient` stores `base_url`, `auth` (static token or OAuth state), and a reusable `reqwest::Client`.
- All requests include `f=json`. Authentication is sent via the `Authorization: Bearer <token>` header (not the `token` query parameter).
- `find_address_candidates`, `reverse_geocode`, and `suggest` use GET query parameters.
- `geocode_addresses` uses POST with `.form(...)`, including `addresses` as serialized JSON string in the form body.
- JSON response parsing routes through `check_and_deserialize`, which first checks for ArcGIS `{ "error": ... }` envelopes.
- For OAuth-managed clients, a `498`/`499` response from any endpoint triggers a single token refresh + retry via `send_with_auth_retry`.

## Logging conventions

Request and response details are logged at `TRACE` level via `log`.

```sh
RUST_LOG=arcgis_geocoder=trace cargo test
```

## Testing conventions

- Use `wiremock` for endpoint tests.
- Start a fresh `MockServer` per test.
- Assert behavior via request matching and typed response assertions.
- Keep tests in `src/lib.rs` under `#[cfg(test)]`.

## Important notes

- When persisting geocoding results, callers must set `for_storage=true` to comply with ArcGIS terms.
- Do not hard-code tokens. Read from environment or a secret manager.
- Coordinates are longitude (`x`) and latitude (`y`).
