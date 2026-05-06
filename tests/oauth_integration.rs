//! Integration tests for the OAuth 2.0 client-credentials flow.
//!
//! These tests make real HTTP requests to `ArcGIS` Online and are skipped
//! automatically when the required environment variables are absent.
//!
//! ## Setup
//!
//! Create a `.env` file at the workspace root (or export the variables):
//!
//! ```text
//! ARCGIS_CLIENT_ID=<your client id>
//! ARCGIS_CLIENT_SECRET=<your client secret>
//! ```
//!
//! Then run with:
//!
//! ```sh
//! cargo test --test oauth_integration
//! ```

use arcgis_geocoder::{
    FindAddressCandidatesParams, GeocoderClient, OAuthCredentials, SuggestParams,
};

/// Loads `.env` and returns the OAuth credentials, or `None` if either
/// `ARCGIS_CLIENT_ID` or `ARCGIS_CLIENT_SECRET` is not set.
fn credentials_from_env() -> Option<OAuthCredentials> {
    dotenvy::dotenv().ok();
    let client_id = std::env::var("ARCGIS_CLIENT_ID").ok()?;
    let client_secret = std::env::var("ARCGIS_CLIENT_SECRET").ok()?;
    Some(OAuthCredentials::new(client_id, client_secret))
}

/// Acquires an access token directly and verifies it is non-empty.
#[tokio::test]
#[ignore = "requires ARCGIS_CLIENT_ID and ARCGIS_CLIENT_SECRET env vars"]
async fn access_token_is_non_empty() {
    let Some(creds) = credentials_from_env() else {
        eprintln!("SKIP: ARCGIS_CLIENT_ID / ARCGIS_CLIENT_SECRET not set");
        return;
    };
    let client = GeocoderClient::with_oauth_credentials(creds).unwrap();
    let token = client
        .access_token()
        .await
        .expect("failed to acquire access token");
    assert!(!token.is_empty(), "access_token must not be empty");
}

/// Calling `access_token()` twice returns the same token (cache hit on the
/// second call — no second round-trip to the token endpoint).
#[tokio::test]
#[ignore = "requires ARCGIS_CLIENT_ID and ARCGIS_CLIENT_SECRET env vars"]
async fn access_token_is_cached() {
    let Some(creds) = credentials_from_env() else {
        eprintln!("SKIP: ARCGIS_CLIENT_ID / ARCGIS_CLIENT_SECRET not set");
        return;
    };
    let client = GeocoderClient::with_oauth_credentials(creds).unwrap();
    let first = client.access_token().await.expect("first token fetch");
    let second = client.access_token().await.expect("second token fetch");
    assert_eq!(first, second, "cached token should match");
}

/// End-to-end geocode: resolves a well-known address and checks the top
/// candidate has a non-empty address string and a score near 100.
#[tokio::test]
#[ignore = "requires ARCGIS_CLIENT_ID and ARCGIS_CLIENT_SECRET env vars"]
async fn find_address_candidates_end_to_end() {
    let Some(creds) = credentials_from_env() else {
        eprintln!("SKIP: ARCGIS_CLIENT_ID / ARCGIS_CLIENT_SECRET not set");
        return;
    };
    let client = GeocoderClient::with_oauth_credentials(creds).unwrap();
    let params = FindAddressCandidatesParams {
        single_line: Some("1600 Pennsylvania Ave NW, Washington, DC".to_owned()),
        out_fields: Some("*".to_owned()),
        max_locations: Some(1),
        ..Default::default()
    };
    let resp = client
        .find_address_candidates(&params)
        .await
        .expect("find_address_candidates failed");
    assert!(
        !resp.candidates.is_empty(),
        "expected at least one candidate"
    );
    let top = &resp.candidates[0];
    assert!(
        !top.address.is_empty(),
        "top candidate address must not be empty"
    );
    assert!(
        top.score >= 90.0,
        "expected high match score, got {}",
        top.score
    );
}

/// End-to-end suggest: returns suggestions for a partial query.
#[tokio::test]
#[ignore = "requires ARCGIS_CLIENT_ID and ARCGIS_CLIENT_SECRET env vars"]
async fn suggest_end_to_end() {
    let Some(creds) = credentials_from_env() else {
        eprintln!("SKIP: ARCGIS_CLIENT_ID / ARCGIS_CLIENT_SECRET not set");
        return;
    };
    let client = GeocoderClient::with_oauth_credentials(creds).unwrap();
    let params = SuggestParams::new("1600 Penn");
    let resp = client.suggest(&params).await.expect("suggest failed");
    assert!(
        !resp.suggestions.is_empty(),
        "expected at least one suggestion"
    );
    let first = &resp.suggestions[0];
    assert!(!first.text.is_empty());
    assert!(!first.magic_key.is_empty());
}
