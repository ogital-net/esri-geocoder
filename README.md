# arcgis-geocoder

An async Rust client for the [ArcGIS World Geocoding Service](https://developers.arcgis.com/documentation/mapping-and-location-services/geocoding/).

Wraps the public REST endpoints with typed request and response models. Built on [reqwest](https://docs.rs/reqwest) with rustls.

## Endpoints

| Method                                    | `ArcGIS` operation        |
| ----------------------------------------- | ------------------------- |
| `GeocoderClient::find_address_candidates` | `findAddressCandidates`   |
| `GeocoderClient::reverse_geocode`         | `reverseGeocode`          |
| `GeocoderClient::suggest`                 | `suggest`                 |
| `GeocoderClient::geocode_addresses`       | `geocodeAddresses` (POST) |

## Installation

```toml
[dependencies]
arcgis-geocoder = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

By default the crate uses [`serde_json`](https://docs.rs/serde_json). To swap
JSON backends see [JSON backend](#json-backend) below.

## Authentication

The client supports two authentication modes.

### Static API key or token

```rust,no_run
# async fn run() -> arcgis_geocoder::Result<()> {
let client = arcgis_geocoder::GeocoderClient::with_token(
    std::env::var("ARCGIS_TOKEN").expect("ARCGIS_TOKEN not set"),
)?;
# Ok(()) }
```

### OAuth 2.0 client credentials

For app authentication via the
[client-credentials flow](https://developers.arcgis.com/documentation/security-and-authentication/app-authentication/client-credentials-flow/).
The client requests an access token on first use and refreshes it before
expiry.

```rust,no_run
use arcgis_geocoder::{GeocoderClient, OAuthCredentials};

# async fn run() -> arcgis_geocoder::Result<()> {
let creds = OAuthCredentials::new(
    std::env::var("ARCGIS_CLIENT_ID").unwrap(),
    std::env::var("ARCGIS_CLIENT_SECRET").unwrap(),
);
let client = GeocoderClient::with_oauth_credentials(creds)?;
# Ok(()) }
```

## Example

```rust,no_run
use arcgis_geocoder::{FindAddressCandidatesParams, GeocoderClient};

#[tokio::main]
async fn main() -> arcgis_geocoder::Result<()> {
    let client = GeocoderClient::with_token(
        std::env::var("ARCGIS_TOKEN").expect("ARCGIS_TOKEN not set"),
    )?;

    let params = FindAddressCandidatesParams {
        single_line: Some("1600 Pennsylvania Ave NW, DC".to_owned()),
        out_fields: Some("*".to_owned()),
        ..Default::default()
    };

    let response = client.find_address_candidates(&params).await?;
    for candidate in &response.candidates {
        println!("{} (score: {})", candidate.address, candidate.score);
    }

    Ok(())
}
```

For batch geocoding, build an `AddressRecordSet` of `AddressRecord` values and
call `geocode_addresses`. `geocodeAddresses` is always billed as a stored
geocode and requires a token with the `premium:user:geocode:stored` privilege.

## Storing results

`ArcGIS` terms require setting `for_storage = true` on requests whose results
will be persisted. This applies to `find_address_candidates` and
`reverse_geocode`. `geocode_addresses` is always treated as stored.

## Configuration

Use `GeocoderClient::builder()` to override the base URL (for testing against
a mock server or pointing at an on-premise locator) or to adjust TLS settings
in development.

```rust,no_run
# fn run() -> arcgis_geocoder::Result<()> {
let client = arcgis_geocoder::GeocoderClient::builder()
    .base_url("https://geocode-api.arcgis.com/arcgis/rest/services/World/GeocodeServer")
    .build("my-token")?;
# Ok(()) }
```

## JSON backend

A JSON backend is selected at compile time. Exactly one of the following
features must be enabled:

| Feature      | Backend                                              |
| ------------ | ---------------------------------------------------- |
| `serde_json` | [`serde_json`](https://docs.rs/serde_json) (default) |
| `sonic-rs`   | [`sonic-rs`](https://docs.rs/sonic-rs)               |
| `simd-json`  | [`simd-json`](https://docs.rs/simd-json)             |

To switch backends, disable the default feature:

```toml
arcgis-geocoder = { version = "0.1", default-features = false, features = ["sonic-rs"] }
```

The active backend's value type is re-exported as `arcgis_geocoder::JsonValue`,
which is the type used for free-form `attributes` maps in response models.

## Logging

Request and response details are logged at the `TRACE` level via the
[`log`](https://docs.rs/log) crate. `TRACE` includes full request and response
bodies.

```sh
RUST_LOG=arcgis_geocoder=trace cargo run
```

## License

Licensed under the BSD 2-Clause License
