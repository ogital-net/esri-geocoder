#![doc = include_str!("../README.md")]

// ── Feature guards ────────────────────────────────────────────────────────────

#[cfg(all(feature = "serde_json", feature = "sonic-rs"))]
compile_error!("features `serde_json` and `sonic-rs` are mutually exclusive; enable only one");
#[cfg(all(feature = "serde_json", feature = "simd-json"))]
compile_error!("features `serde_json` and `simd-json` are mutually exclusive; enable only one");
#[cfg(all(feature = "sonic-rs", feature = "simd-json"))]
compile_error!("features `sonic-rs` and `simd-json` are mutually exclusive; enable only one");
#[cfg(not(any(feature = "serde_json", feature = "sonic-rs", feature = "simd-json")))]
compile_error!(
    "at least one of the `serde_json`, `sonic-rs`, or `simd-json` features must be enabled"
);

// ── JSON backend shim ─────────────────────────────────────────────────────────

/// Active JSON backend, selected at compile time by the `serde_json`, `sonic-rs`,
/// or `simd-json` feature flag.
#[cfg(feature = "serde_json")]
mod json {
    pub use serde_json::{from_slice, to_vec, Error, Value};

    pub fn to_value<T: serde::Serialize>(v: &T) -> Value {
        // Serializing primitive types into a JSON Value is infallible.
        serde_json::to_value(v).expect("infallible primitive serialization")
    }
}

#[cfg(feature = "sonic-rs")]
mod json {
    pub use sonic_rs::{from_slice, to_vec, Error, Value};

    pub fn to_value<T: serde::Serialize>(v: &T) -> Value {
        // Serializing primitive types into a JSON Value is infallible.
        sonic_rs::to_value(v).expect("infallible primitive serialization")
    }
}

#[cfg(feature = "simd-json")]
mod json {
    pub use simd_json::{to_vec, Error, OwnedValue as Value};
    pub fn from_slice<T>(input: &[u8]) -> Result<T, Error>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        let mut bytes = input.to_vec();
        simd_json::serde::from_slice(&mut bytes)
    }

    pub fn to_value<T: serde::Serialize>(v: &T) -> Value {
        // Serializing primitive types into a JSON Value is infallible.
        simd_json::serde::to_owned_value(v).expect("infallible primitive serialization")
    }
}

/// The JSON value type provided by the active JSON backend.
pub use json::Value as JsonValue;

/// The JSON error type provided by the active JSON backend.
///
/// **Note**: this alias is backend-specific. Code that pattern-matches on the
/// inner error type is not portable across the `serde_json`, `sonic-rs`, and
/// `simd-json` features.
pub use json::Error as JsonError;

// ── Imports ───────────────────────────────────────────────────────────────────

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{log_enabled, trace, Level};
use serde::Deserialize;
use tokio::sync::Mutex;

mod models;
pub use models::*;

// ── Public types ──────────────────────────────────────────────────────────────

pub type Result<T> = std::result::Result<T, Error>;

/// Crate-level error type.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An HTTP transport error.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// A non-zero error code returned in the `ArcGIS` API response body.
    #[error("API error {code}: {message}{}", format_details(details))]
    Api {
        code: i32,
        message: String,
        /// Additional per-component error details supplied by `ArcGIS`. May be empty.
        details: Vec<String>,
    },

    /// A JSON deserialization error on the response body.
    #[error("JSON error: {0}")]
    Json(#[from] json::Error),
}

fn format_details(details: &[String]) -> String {
    if details.is_empty() {
        String::new()
    } else {
        format!(" ({})", details.join("; "))
    }
}

// ── ArcGIS error envelope ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ArcGisErrorBody {
    code: i32,
    message: String,
    #[serde(default)]
    details: Vec<String>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    #[serde(default)]
    error: Option<ArcGisErrorBody>,
}

// ── Debug logging helpers ─────────────────────────────────────────────────────

fn log_request(req: &reqwest::Request) {
    trace!("--> {} {}", req.method(), req.url());
    for (name, value) in req.headers() {
        trace!("    {}: {}", name, value.to_str().unwrap_or("<binary>"));
    }
    if let Some(bytes) = req.body().and_then(reqwest::Body::as_bytes) {
        if !bytes.is_empty() {
            trace!("    {}", String::from_utf8_lossy(bytes));
        }
    }
}

fn log_response_head(resp: &reqwest::Response) {
    trace!("<-- {}", resp.status());
    for (name, value) in resp.headers() {
        trace!("    {}: {}", name, value.to_str().unwrap_or("<binary>"));
    }
}

// ── Response deserialization ──────────────────────────────────────────────────

/// Checks the response bytes for an `ArcGIS` error envelope, then deserializes
/// into `T`. Called by both `send_json` and the form-POST `geocode_addresses` path.
fn check_and_deserialize<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    if let Ok(env) = json::from_slice::<ErrorEnvelope>(bytes) {
        if let Some(err) = env.error {
            return Err(Error::Api {
                code: err.code,
                message: err.message,
                details: err.details,
            });
        }
    }
    json::from_slice::<T>(bytes).map_err(Error::Json)
}

// ── Query-parameter helpers ──────────────────────────────────────────────────

/// Pushes optional `(key, value)` query/form parameters onto a
/// `Vec<(&'static str, Cow<'_, str>)>`. Used by every endpoint method.
macro_rules! push_param {
    ($q:ident, str, $key:literal, $opt:expr) => {
        if let Some(v) = &$opt {
            $q.push(($key, Cow::Borrowed(v.as_str())));
        }
    };
    ($q:ident, bool, $key:literal, $opt:expr) => {
        if let Some(v) = $opt {
            $q.push(($key, Cow::Borrowed(if v { "true" } else { "false" })));
        }
    };
    ($q:ident, num, $key:literal, $opt:expr) => {
        if let Some(v) = $opt {
            $q.push(($key, Cow::Owned(v.to_string())));
        }
    };
}

// ── RequestBuilderExt ─────────────────────────────────────────────────────────

trait RequestBuilderExt {
    /// Send the request and deserialize the JSON response body via the active
    /// JSON backend. Logs the full request and response at `TRACE` level.
    async fn send_json<T>(self) -> Result<T>
    where
        T: for<'de> Deserialize<'de>;
}

impl RequestBuilderExt for reqwest::RequestBuilder {
    async fn send_json<T>(self) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let do_log = log_enabled!(Level::Trace);

        if do_log {
            if let Some(snapshot) = self.try_clone() {
                if let Ok(req) = snapshot.build() {
                    log_request(&req);
                }
            }
        }

        let resp = self.send().await?;

        if do_log {
            log_response_head(&resp);
        }

        let bytes = resp.bytes().await?;

        if do_log && !bytes.is_empty() {
            trace!("    {}", String::from_utf8_lossy(&bytes));
        }

        check_and_deserialize(&bytes)
    }
}

// ── ClientBuilder ─────────────────────────────────────────────────────────────

/// Default `ArcGIS` World Geocoding Service base URL.
pub const DEFAULT_BASE_URL: &str =
    "https://geocode-api.arcgis.com/arcgis/rest/services/World/GeocodeServer";

/// Default `ArcGIS` OAuth 2.0 token endpoint used by the client-credentials flow.
pub const DEFAULT_TOKEN_URL: &str = "https://www.arcgis.com/sharing/rest/oauth2/token";

/// Refresh tokens this many seconds before their reported expiry to avoid
/// races against in-flight requests.
const TOKEN_REFRESH_SKEW: Duration = Duration::from_secs(60);

/// Fallback lifetime applied only when the token endpoint reports `expires_in = 0`
/// (or omits the field). Positive values are honoured as-is.
const MIN_TOKEN_LIFETIME: Duration = Duration::from_secs(300);

/// `ArcGIS` OAuth 2.0 application credentials used by the client-credentials flow.
///
/// The [`Debug`] impl redacts the client id and secret to avoid leaking
/// credentials through logs.
///
/// See [Client credentials flow](https://developers.arcgis.com/documentation/security-and-authentication/app-authentication/client-credentials-flow/).
#[derive(Clone)]
pub struct OAuthCredentials {
    client_id: String,
    client_secret: String,
    token_url: Option<String>,
}

impl std::fmt::Debug for OAuthCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthCredentials")
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .field("token_url", &self.token_url)
            .finish()
    }
}

impl OAuthCredentials {
    /// Creates a new credentials value targeting the default `ArcGIS` token
    /// endpoint ([`DEFAULT_TOKEN_URL`]).
    #[must_use]
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            token_url: None,
        }
    }

    /// Override the token endpoint (e.g. for `ArcGIS` Enterprise deployments).
    #[must_use]
    pub fn with_token_url(mut self, url: impl Into<String>) -> Self {
        self.token_url = Some(url.into());
        self
    }

    /// Returns the configured token endpoint, or [`DEFAULT_TOKEN_URL`] if none
    /// was set.
    #[must_use]
    pub fn token_url(&self) -> &str {
        self.token_url.as_deref().unwrap_or(DEFAULT_TOKEN_URL)
    }
}

/// Token-endpoint success response (`{ "access_token": "...", "expires_in": N }`).
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

/// OAuth 2.0 spec error envelope (`{ "error": "...", "error_description": "..." }`)
/// used by some `ArcGIS` token-endpoint error paths in lieu of the `f=json`
/// envelope `{"error": {"code": ...}}`.
#[derive(Deserialize)]
struct OAuthErrorEnvelope {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Clone)]
struct CachedToken {
    access_token: Arc<str>,
    expires_at: Instant,
}

struct OAuthState {
    client_id: String,
    client_secret: String,
    token_url: String,
    /// Single-flight refresh + cache. Held across the network `await` so that
    /// concurrent callers issue at most one refresh request.
    cache: Mutex<Option<CachedToken>>,
}

impl std::fmt::Debug for OAuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthState")
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .field("token_url", &self.token_url)
            .finish_non_exhaustive()
    }
}

/// Authentication strategy used by [`GeocoderClient`].
#[derive(Clone)]
enum Auth {
    /// A pre-acquired API key or OAuth access token, used as-is.
    Static(Arc<str>),
    /// OAuth client-credentials flow: tokens are acquired and refreshed on demand.
    OAuth(Arc<OAuthState>),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::Static(_) => f.write_str("Static(<redacted>)"),
            Auth::OAuth(state) => f.debug_tuple("OAuth").field(state).finish(),
        }
    }
}

impl Auth {
    /// Returns a currently-valid access token, refreshing via the token endpoint
    /// if the cached value is missing or near expiry.
    ///
    /// The refresh path is single-flighted via a `tokio::sync::Mutex`:
    /// concurrent callers serialize on the cache lock, so at most one refresh
    /// request is in flight at a time.
    async fn token(&self, http: &reqwest::Client) -> Result<Arc<str>> {
        match self {
            Auth::Static(t) => Ok(Arc::clone(t)),
            Auth::OAuth(state) => {
                let mut guard = state.cache.lock().await;
                if let Some(cached) = guard.as_ref() {
                    if Instant::now() + TOKEN_REFRESH_SKEW < cached.expires_at {
                        return Ok(Arc::clone(&cached.access_token));
                    }
                }

                let new_cached = fetch_token(http, state).await?;
                let token = Arc::clone(&new_cached.access_token);
                *guard = Some(new_cached);
                Ok(token)
            }
        }
    }
}

/// Performs a token-endpoint POST and parses the response. Inspects both the
/// `f=json` envelope (`{"error":{"code":…}}`) and the OAuth-spec envelope
/// (`{"error":"invalid_client",…}`) so credential errors surface as
/// [`Error::Api`] rather than [`Error::Json`].
async fn fetch_token(http: &reqwest::Client, state: &OAuthState) -> Result<CachedToken> {
    let resp = http
        .post(&state.token_url)
        .form(&[
            ("f", "json"),
            ("grant_type", "client_credentials"),
            ("client_id", state.client_id.as_str()),
            ("client_secret", state.client_secret.as_str()),
        ])
        .send()
        .await?;
    let bytes = resp.bytes().await?;

    // Try the `f=json` envelope first (handled by the shared helper).
    if let Ok(env) = json::from_slice::<ErrorEnvelope>(&bytes) {
        if let Some(err) = env.error {
            return Err(Error::Api {
                code: err.code,
                message: err.message,
                details: err.details,
            });
        }
    }
    // Then the OAuth-spec error envelope.
    if let Ok(env) = json::from_slice::<OAuthErrorEnvelope>(&bytes) {
        let message = env.error_description.unwrap_or_else(|| env.error.clone());
        return Err(Error::Api {
            code: 0,
            message: format!("oauth: {} ({message})", env.error),
            details: Vec::new(),
        });
    }

    let resp: TokenResponse = json::from_slice(&bytes).map_err(Error::Json)?;
    // Only apply the lifetime floor when the endpoint omits or zeros `expires_in`;
    // honour any positive value the IdP actually reports (it may legitimately
    // issue short-lived tokens).
    let lifetime = if resp.expires_in == 0 {
        MIN_TOKEN_LIFETIME
    } else {
        Duration::from_secs(resp.expires_in)
    };
    Ok(CachedToken {
        access_token: Arc::from(resp.access_token),
        expires_at: Instant::now() + lifetime,
    })
}

/// Builder for configuring and constructing a [`GeocoderClient`].
///
/// Obtain one via [`GeocoderClient::builder`].
///
/// # Example
///
/// ```no_run
/// # fn main() -> arcgis_geocoder::Result<()> {
/// let client = arcgis_geocoder::GeocoderClient::builder()
///     .danger_accept_invalid_certs(true)
///     .build("my-token")?;
/// # Ok(()) }
/// ```
/// Default `User-Agent` header sent on every request.
pub const DEFAULT_USER_AGENT: &str = concat!("arcgis-geocoder/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Default)]
pub struct ClientBuilder {
    accept_invalid_certs: bool,
    base_url: Option<String>,
    http_client: Option<reqwest::Client>,
}

impl ClientBuilder {
    /// Creates a new builder with default settings (TLS verification enabled,
    /// standard `ArcGIS` endpoint).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Disables TLS certificate verification.
    ///
    /// **Security warning**: only use on trusted private networks.
    ///
    /// Ignored when a pre-built HTTP client is supplied via
    /// [`http_client`](Self::http_client).
    #[must_use]
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Override the geocoder base URL (defaults to the standard `ArcGIS` endpoint).
    #[must_use]
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Use a pre-configured [`reqwest::Client`] instead of having the builder
    /// construct one. Useful for sharing a connection pool, configuring
    /// proxies, custom timeouts, or middleware.
    ///
    /// When set, [`danger_accept_invalid_certs`](Self::danger_accept_invalid_certs)
    /// is ignored — configure TLS on the supplied client directly. Callers are
    /// also responsible for setting an appropriate `User-Agent`.
    #[must_use]
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }

    fn build_http(&self) -> Result<reqwest::Client> {
        if let Some(client) = &self.http_client {
            return Ok(client.clone());
        }
        reqwest::Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .danger_accept_invalid_certs(self.accept_invalid_certs)
            .build()
            .map_err(Error::Http)
    }

    fn resolved_base_url(&self) -> String {
        self.base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned())
    }

    /// Constructs the [`GeocoderClient`] with the given `ArcGIS` API key or
    /// pre-acquired OAuth access token.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be built.
    pub fn build(self, token: impl Into<String>) -> Result<GeocoderClient> {
        let http = self.build_http()?;
        let base_url = self.resolved_base_url();
        Ok(GeocoderClient {
            base_url,
            auth: Auth::Static(Arc::from(token.into())),
            http,
        })
    }

    /// Constructs the [`GeocoderClient`] using OAuth 2.0 client-credentials.
    ///
    /// The client will request an access token on first use and refresh it
    /// automatically before expiry.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be built.
    pub fn build_oauth(self, credentials: OAuthCredentials) -> Result<GeocoderClient> {
        let http = self.build_http()?;
        let base_url = self.resolved_base_url();
        let token_url = credentials
            .token_url
            .unwrap_or_else(|| DEFAULT_TOKEN_URL.to_owned());
        let state = OAuthState {
            client_id: credentials.client_id,
            client_secret: credentials.client_secret,
            token_url,
            cache: Mutex::new(None),
        };
        Ok(GeocoderClient {
            base_url,
            auth: Auth::OAuth(Arc::new(state)),
            http,
        })
    }
}

// ── GeocoderClient ────────────────────────────────────────────────────────────

/// Async client for the `ArcGIS` World Geocoding Service.
///
/// Holds a base URL, authentication state, and persistent HTTP connection
/// pool. Construct via [`GeocoderClient::with_token`],
/// [`GeocoderClient::with_oauth_credentials`], or [`GeocoderClient::builder`].
///
/// When constructed with OAuth client credentials the client manages the
/// access-token lifecycle automatically, requesting a new token on first use
/// and refreshing it before expiry.
///
/// Credentials are redacted from the [`Debug`] output to avoid leaking secrets
/// through logs.
#[derive(Clone)]
pub struct GeocoderClient {
    base_url: String,
    auth: Auth,
    http: reqwest::Client,
}

impl std::fmt::Debug for GeocoderClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let auth_kind = match &self.auth {
            Auth::Static(_) => "static",
            Auth::OAuth(_) => "oauth-client-credentials",
        };
        f.debug_struct("GeocoderClient")
            .field("base_url", &self.base_url)
            .field("auth", &auth_kind)
            .field("http", &self.http)
            .finish()
    }
}

impl GeocoderClient {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Returns a [`ClientBuilder`] for advanced configuration (e.g. custom base
    /// URL, TLS options). See [`ClientBuilder`] for a full usage example.
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Constructs a client using the standard `ArcGIS` endpoint with a static
    /// API key or pre-acquired OAuth access token.
    ///
    /// For automatic OAuth token management see
    /// [`GeocoderClient::with_oauth_credentials`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be built.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # #[tokio::main]
    /// # async fn main() -> arcgis_geocoder::Result<()> {
    /// let client = arcgis_geocoder::GeocoderClient::with_token("my-api-key")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_token(token: impl Into<String>) -> Result<Self> {
        Self::builder().build(token)
    }

    /// Constructs a client using OAuth 2.0 client-credentials. The client
    /// requests an access token on first use and refreshes it automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be built.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # #[tokio::main]
    /// # async fn main() -> arcgis_geocoder::Result<()> {
    /// use arcgis_geocoder::{GeocoderClient, OAuthCredentials};
    /// let creds = OAuthCredentials::new(
    ///     std::env::var("ARCGIS_CLIENT_ID").unwrap(),
    ///     std::env::var("ARCGIS_CLIENT_SECRET").unwrap(),
    /// );
    /// let client = GeocoderClient::with_oauth_credentials(creds)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_oauth_credentials(credentials: OAuthCredentials) -> Result<Self> {
        Self::builder().build_oauth(credentials)
    }

    /// Returns a currently-valid access token, refreshing via the OAuth token
    /// endpoint when necessary. For static tokens this simply returns a clone.
    ///
    /// # Errors
    ///
    /// Returns an error if a token refresh request fails.
    pub async fn access_token(&self) -> Result<Arc<str>> {
        self.auth.token(&self.http).await
    }

    /// Forces the cached OAuth token (if any) to expire. No-op for static
    /// tokens.
    async fn invalidate_token(&self) {
        if let Auth::OAuth(state) = &self.auth {
            *state.cache.lock().await = None;
        }
    }

    /// Test-only alias kept for clarity in integration tests.
    #[cfg(test)]
    async fn force_token_refresh(&self) {
        self.invalidate_token().await;
    }

    /// Calls `build_req` to produce a fully-configured `RequestBuilder` for the
    /// given access token, sends it, and on a `498`/`499` (invalid/expired
    /// token) response from an OAuth-managed client, invalidates the cached
    /// token and retries once.
    async fn send_with_auth_retry<B, T>(&self, build_req: B) -> Result<T>
    where
        B: Fn(&str) -> reqwest::RequestBuilder,
        T: for<'de> Deserialize<'de>,
    {
        let token = self.access_token().await?;
        let result = build_req(&token).send_json::<T>().await;
        match result {
            Err(Error::Api {
                code: 498 | 499, ..
            }) if matches!(self.auth, Auth::OAuth(_)) => {
                self.invalidate_token().await;
                let token = self.access_token().await?;
                build_req(&token).send_json::<T>().await
            }
            other => other,
        }
    }

    // ── findAddressCandidates ─────────────────────────────────────────────────

    /// Forward geocoding — converts address text to one or more candidates with
    /// locations and match scores.
    ///
    /// Set [`FindAddressCandidatesParams::for_storage`] to `true` when the
    /// results will be persisted; omitting it for stored results violates the
    /// `ArcGIS` Terms of Use.
    ///
    /// To resolve a [`Suggestion`] from [`suggest`](Self::suggest), set
    /// [`FindAddressCandidatesParams::magic_key`] to the suggestion's
    /// `magic_key` and pass the same partial text via
    /// [`FindAddressCandidatesParams::single_line`]; the API requires both.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the API returns an error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> arcgis_geocoder::Result<()> {
    /// let client = arcgis_geocoder::GeocoderClient::with_token("my-api-key")?;
    /// let params = arcgis_geocoder::FindAddressCandidatesParams {
    ///     single_line: Some("1600 Pennsylvania Ave NW, DC".to_owned()),
    ///     out_fields: Some("*".to_owned()),
    ///     ..Default::default()
    /// };
    /// let resp = client.find_address_candidates(&params).await?;
    /// for c in &resp.candidates {
    ///     println!("{}: {}", c.address, c.score);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn find_address_candidates(
        &self,
        params: &FindAddressCandidatesParams,
    ) -> Result<FindAddressCandidatesResponse> {
        let url = format!("{}/findAddressCandidates", self.base_url);

        let mut q: Vec<(&str, Cow<str>)> = vec![("f", "json".into())];

        push_param!(q, str, "singleLine", params.single_line);
        push_param!(q, str, "address", params.address);
        push_param!(q, str, "address2", params.address2);
        push_param!(q, str, "address3", params.address3);
        push_param!(q, str, "neighborhood", params.neighborhood);
        push_param!(q, str, "city", params.city);
        push_param!(q, str, "subregion", params.subregion);
        push_param!(q, str, "region", params.region);
        push_param!(q, str, "postal", params.postal);
        push_param!(q, str, "postalExt", params.postal_ext);
        push_param!(q, str, "countryCode", params.country_code);
        push_param!(q, str, "category", params.category);
        push_param!(q, str, "outFields", params.out_fields);
        push_param!(q, num, "outSR", params.out_sr);
        push_param!(q, num, "maxLocations", params.max_locations);
        push_param!(q, str, "searchExtent", params.search_extent);
        push_param!(q, str, "location", params.location);
        push_param!(q, str, "langCode", params.lang_code);
        push_param!(q, str, "locationType", params.location_type);
        push_param!(q, str, "sourceCountry", params.source_country);
        push_param!(
            q,
            str,
            "preferredLabelValues",
            params.preferred_label_values
        );
        push_param!(q, bool, "matchOutOfRange", params.match_out_of_range);
        push_param!(q, bool, "forStorage", params.for_storage);
        push_param!(
            q,
            bool,
            "comprehensiveZoneMatch",
            params.comprehensive_zone_match
        );
        push_param!(
            q,
            bool,
            "returnMatchNarrative",
            params.return_match_narrative
        );
        push_param!(
            q,
            bool,
            "returnPrimaryMatchID",
            params.return_primary_match_id
        );
        push_param!(
            q,
            str,
            "excludeIntersectionType",
            params.exclude_intersection_type
        );
        push_param!(q, str, "matchID", params.match_id);
        push_param!(q, str, "searchWithin", params.search_within);
        push_param!(q, num, "start", params.start);
        push_param!(q, num, "num", params.num);
        push_param!(q, str, "magicKey", params.magic_key);

        self.send_with_auth_retry(|token| self.http.get(&url).query(&q).bearer_auth(token))
            .await
    }

    // ── reverseGeocode ────────────────────────────────────────────────────────

    /// Reverse geocoding — converts a geographic point to the nearest address
    /// or place.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the API returns an error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> arcgis_geocoder::Result<()> {
    /// let client = arcgis_geocoder::GeocoderClient::with_token("my-api-key")?;
    /// let params = arcgis_geocoder::ReverseGeocodeParams::new("-77.036556,38.897663");
    /// let resp = client.reverse_geocode(&params).await?;
    /// println!("{:?}", resp.address.get("LongLabel"));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reverse_geocode(
        &self,
        params: &ReverseGeocodeParams,
    ) -> Result<ReverseGeocodeResponse> {
        let url = format!("{}/reverseGeocode", self.base_url);

        let mut q: Vec<(&str, Cow<str>)> = vec![
            ("f", "json".into()),
            ("location", Cow::Borrowed(&params.location)),
        ];

        push_param!(q, str, "featureTypes", params.feature_types);
        push_param!(q, str, "locationType", params.location_type);
        push_param!(q, str, "langCode", params.lang_code);
        push_param!(q, bool, "returnIntersection", params.return_intersection);
        push_param!(q, bool, "forStorage", params.for_storage);
        push_param!(q, num, "outSR", params.out_sr);
        push_param!(
            q,
            str,
            "preferredLabelValues",
            params.preferred_label_values
        );
        push_param!(q, str, "outFields", params.out_fields);
        push_param!(q, bool, "returnInputLocation", params.return_input_location);

        self.send_with_auth_retry(|token| self.http.get(&url).query(&q).bearer_auth(token))
            .await
    }

    // ── suggest ───────────────────────────────────────────────────────────────

    /// Autosuggest — returns up to 15 candidate suggestions (default 5) for
    /// partial address or place-name text.
    ///
    /// Pass the returned [`Suggestion::magic_key`] to
    /// [`find_address_candidates`](Self::find_address_candidates) to resolve
    /// the full address and location.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the API returns an error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> arcgis_geocoder::Result<()> {
    /// let client = arcgis_geocoder::GeocoderClient::with_token("my-api-key")?;
    /// let params = arcgis_geocoder::SuggestParams::new("1600 Penn");
    /// let resp = client.suggest(&params).await?;
    /// for s in &resp.suggestions {
    ///     println!("{} (key: {})", s.text, s.magic_key);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn suggest(&self, params: &SuggestParams) -> Result<SuggestResponse> {
        let url = format!("{}/suggest", self.base_url);

        let mut q: Vec<(&str, Cow<str>)> =
            vec![("f", "json".into()), ("text", Cow::Borrowed(&params.text))];

        push_param!(q, str, "location", params.location);
        push_param!(q, str, "searchExtent", params.search_extent);
        push_param!(q, str, "category", params.category);
        push_param!(q, str, "countryCode", params.country_code);
        push_param!(q, num, "maxSuggestions", params.max_suggestions);
        push_param!(q, str, "sourceCountry", params.source_country);
        push_param!(
            q,
            str,
            "preferredLabelValues",
            params.preferred_label_values
        );
        push_param!(q, bool, "returnCollections", params.return_collections);
        push_param!(q, bool, "partialHouseNumber", params.partial_house_number);
        push_param!(q, bool, "partialSubaddress", params.partial_subaddress);
        push_param!(q, bool, "subaddressSummary", params.subaddress_summary);
        push_param!(
            q,
            bool,
            "subaddressAfterBaseAddress",
            params.subaddress_after_base_address
        );

        self.send_with_auth_retry(|token| self.http.get(&url).query(&q).bearer_auth(token))
            .await
    }

    // ── geocodeAddresses ──────────────────────────────────────────────────────

    /// Batch geocoding — geocodes a list of addresses in a single POST request.
    ///
    /// Suitable for small-to-medium batches (up to ~1 000 records). Always
    /// billed as stored geocodes; requires a token with the
    /// `premium:user:geocode:stored` privilege.
    ///
    /// The [`GeocodeLocation::result_id`] in each response matches the
    /// `objectid` attribute of the corresponding input [`AddressRecord`].
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the API returns an error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> arcgis_geocoder::Result<()> {
    /// use arcgis_geocoder::{AddressRecord, AddressRecordSet, GeocodeAddressesParams, GeocoderClient};
    /// let client = GeocoderClient::with_token("my-api-key")?;
    /// let params = GeocodeAddressesParams {
    ///     addresses: AddressRecordSet::from_records([
    ///         AddressRecord::single_line(1, "380 New York St, Redlands, CA"),
    ///         AddressRecord::single_line(2, "1 World Way, Los Angeles, CA"),
    ///     ]),
    ///     ..Default::default()
    /// };
    /// let resp = client.geocode_addresses(&params).await?;
    /// println!("{} locations returned", resp.locations.len());
    /// # Ok(())
    /// # }
    /// ```
    #[allow(clippy::missing_panics_doc)] // Only panics if the JSON backend produces non-UTF-8.
    pub async fn geocode_addresses(
        &self,
        params: &GeocodeAddressesParams,
    ) -> Result<GeocodeAddressesResponse> {
        let url = format!("{}/geocodeAddresses", self.base_url);

        let addresses_bytes = json::to_vec(&params.addresses).map_err(Error::Json)?;
        let addresses_str =
            String::from_utf8(addresses_bytes).expect("JSON serialization always produces UTF-8");

        let mut form: Vec<(&str, Cow<str>)> = vec![
            ("f", "json".into()),
            ("addresses", Cow::Owned(addresses_str)),
        ];

        push_param!(form, str, "sourceCountry", params.source_country);
        push_param!(form, str, "searchExtent", params.search_extent);
        push_param!(form, str, "locationType", params.location_type);
        push_param!(form, str, "category", params.category);
        push_param!(form, str, "langCode", params.lang_code);
        push_param!(form, str, "outFields", params.out_fields);
        push_param!(form, num, "outSR", params.out_sr);
        push_param!(form, bool, "matchOutOfRange", params.match_out_of_range);
        push_param!(
            form,
            str,
            "preferredLabelValues",
            params.preferred_label_values
        );
        push_param!(
            form,
            bool,
            "comprehensiveZoneMatch",
            params.comprehensive_zone_match
        );
        push_param!(
            form,
            bool,
            "returnMatchNarrative",
            params.return_match_narrative
        );
        push_param!(
            form,
            str,
            "excludeIntersectionType",
            params.exclude_intersection_type
        );

        self.send_with_auth_retry(|token| self.http.post(&url).form(&form).bearer_auth(token))
            .await
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::Serialize;
    use wiremock::{
        matchers::{header, method, path, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;

    const TOKEN: &str = "test-token-abc";

    fn make_client(base_url: impl Into<String>) -> GeocoderClient {
        GeocoderClient::builder()
            .base_url(base_url)
            .build(TOKEN)
            .unwrap()
    }

    // ── find_address_candidates ───────────────────────────────────────────────

    /// GET /findAddressCandidates?f=json&token=...&singleLine=...
    /// Returns a list of address candidates.
    #[tokio::test]
    async fn find_address_candidates_returns_candidates() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .and(query_param("f", "json"))
            .and(header("authorization", &*format!("Bearer {TOKEN}")))
            .and(query_param("singleLine", "1600 Pennsylvania Ave NW, DC"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "spatialReference": { "wkid": 4326, "latestWkid": 4326 },
                "candidates": [
                    {
                        "address": "1600 Pennsylvania Ave NW, Washington, DC 20500",
                        "location": { "x": -77.036_556, "y": 38.897_663 },
                        "score": 100.0,
                        "attributes": {}
                    },
                    {
                        "address": "1600 Pennsylvania Ave NW, Washington, DC 20006",
                        "location": { "x": -77.040_123, "y": 38.895_000 },
                        "score": 92.5,
                        "attributes": {}
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("1600 Pennsylvania Ave NW, DC".to_owned()),
            ..Default::default()
        };

        let resp = client.find_address_candidates(&params).await.unwrap();

        assert_eq!(resp.candidates.len(), 2);
        assert_eq!(
            resp.candidates[0].address,
            "1600 Pennsylvania Ave NW, Washington, DC 20500"
        );
        assert!((resp.candidates[0].location.x - (-77.036_556)).abs() < 1e-6);
        assert!((resp.candidates[0].location.y - 38.897_663).abs() < 1e-6);
        assert!((resp.candidates[0].score - 100.0).abs() < 1e-6);
        assert!(resp.candidates[1].score < resp.candidates[0].score);
    }

    /// Multi-field address components are forwarded as separate query params.
    #[tokio::test]
    async fn find_address_candidates_multi_field_params() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .and(query_param("address", "1600 Pennsylvania Ave NW"))
            .and(query_param("city", "Washington"))
            .and(query_param("region", "DC"))
            .and(query_param("postal", "20500"))
            .and(query_param("countryCode", "USA"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [
                    {
                        "address": "1600 Pennsylvania Ave NW, Washington, DC 20500",
                        "location": { "x": -77.036_556, "y": 38.897_663 },
                        "score": 100.0,
                        "attributes": {}
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = FindAddressCandidatesParams {
            address: Some("1600 Pennsylvania Ave NW".to_owned()),
            city: Some("Washington".to_owned()),
            region: Some("DC".to_owned()),
            postal: Some("20500".to_owned()),
            country_code: Some("USA".to_owned()),
            ..Default::default()
        };

        let resp = client.find_address_candidates(&params).await.unwrap();
        assert_eq!(resp.candidates.len(), 1);
    }

    /// outFields, searchExtent, location, langCode, forStorage, searchWithin,
    /// and magicKey are all forwarded when set.
    #[tokio::test]
    async fn find_address_candidates_optional_params_forwarded() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .and(query_param("outFields", "*"))
            .and(query_param("searchExtent", "-80,35,-70,45"))
            .and(query_param("location", "-77.0,38.9"))
            .and(query_param("langCode", "en"))
            .and(query_param("forStorage", "true"))
            .and(query_param("searchWithin", "POI"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": []
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("coffee".to_owned()),
            out_fields: Some("*".to_owned()),
            search_extent: Some("-80,35,-70,45".to_owned()),
            location: Some("-77.0,38.9".to_owned()),
            lang_code: Some("en".to_owned()),
            for_storage: Some(true),
            search_within: Some("POI".to_owned()),
            ..Default::default()
        };

        let resp = client.find_address_candidates(&params).await.unwrap();
        assert_eq!(resp.candidates.len(), 0);
    }

    /// A `magic_key` from a prior suggest call is forwarded to resolve the suggestion.
    #[tokio::test]
    async fn find_address_candidates_with_magic_key() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .and(query_param("magicKey", "dHA9MCNsb2M9MA=="))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [
                    {
                        "address": "New York, NY, USA",
                        "location": { "x": -74.0060, "y": 40.7128 },
                        "score": 100.0,
                        "attributes": {}
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("New Y".to_owned()),
            magic_key: Some("dHA9MCNsb2M9MA==".to_owned()),
            ..Default::default()
        };

        let resp = client.find_address_candidates(&params).await.unwrap();
        assert_eq!(resp.candidates[0].address, "New York, NY, USA");
    }

    /// An `ArcGIS` error response surfaces as `Error::Api`.
    #[tokio::test]
    async fn find_address_candidates_api_error() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": {
                    "code": 498,
                    "message": "Invalid token.",
                    "details": []
                }
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("anything".to_owned()),
            ..Default::default()
        };

        let err = client.find_address_candidates(&params).await.unwrap_err();
        match err {
            Error::Api { code, .. } => assert_eq!(code, 498),
            other => panic!("expected Error::Api, got {other:?}"),
        }
    }

    // ── reverse_geocode ───────────────────────────────────────────────────────

    /// GET /reverseGeocode?f=json&token=...&location=...
    /// Returns an address for the given location.
    #[tokio::test]
    async fn reverse_geocode_returns_address() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/reverseGeocode"))
            .and(query_param("f", "json"))
            .and(header("authorization", &*format!("Bearer {TOKEN}")))
            .and(query_param("location", "-77.036556,38.897663"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "address": {
                    "AddNum": "1600",
                    "Street": "Pennsylvania Ave NW",
                    "City": "Washington",
                    "Region": "District of Columbia",
                    "Postal": "20500",
                    "CountryCode": "USA",
                    "LongLabel": "1600 Pennsylvania Ave NW, Washington, DC 20500, USA"
                },
                "location": { "x": -77.036_556, "y": 38.897_663 }
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = ReverseGeocodeParams::new("-77.036556,38.897663");

        let resp = client.reverse_geocode(&params).await.unwrap();

        assert!(resp.address.contains_key("LongLabel"));
        assert!(resp.address.contains_key("City"));
        assert!((resp.location.x - (-77.036_556)).abs() < 1e-6);
    }

    /// Optional reverse geocode params are forwarded when set.
    #[tokio::test]
    async fn reverse_geocode_optional_params_forwarded() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/reverseGeocode"))
            .and(query_param("featureTypes", "StreetAddress"))
            .and(query_param("locationType", "rooftop"))
            .and(query_param("returnIntersection", "false"))
            .and(query_param("langCode", "en"))
            .and(query_param("forStorage", "false"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "address": { "LongLabel": "Test St, City, ST 12345" },
                "location": { "x": -100.0, "y": 40.0 }
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = ReverseGeocodeParams {
            location: "-100.0,40.0".to_owned(),
            feature_types: Some("StreetAddress".to_owned()),
            location_type: Some("rooftop".to_owned()),
            return_intersection: Some(false),
            lang_code: Some("en".to_owned()),
            for_storage: Some(false),
            ..Default::default()
        };

        let resp = client.reverse_geocode(&params).await.unwrap();
        assert!(resp.address.contains_key("LongLabel"));
    }

    /// An `ArcGIS` error from `reverse_geocode` surfaces as `Error::Api`.
    #[tokio::test]
    async fn reverse_geocode_api_error() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/reverseGeocode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": {
                    "code": 400,
                    "message": "Unable to find address for the given location.",
                    "details": []
                }
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = ReverseGeocodeParams::new("0.0,0.0");

        let err = client.reverse_geocode(&params).await.unwrap_err();
        match err {
            Error::Api { code, .. } => assert_eq!(code, 400),
            other => panic!("expected Error::Api, got {other:?}"),
        }
    }

    // ── suggest ───────────────────────────────────────────────────────────────

    /// GET /suggest?f=json&token=...&text=...
    /// Returns up to 5 suggestions.
    #[tokio::test]
    async fn suggest_returns_suggestions() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/suggest"))
            .and(query_param("f", "json"))
            .and(header("authorization", &*format!("Bearer {TOKEN}")))
            .and(query_param("text", "New Y"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "suggestions": [
                    {
                        "text": "New York, NY, USA",
                        "magicKey": "dHA9MCNsb2M9MA==",
                        "isCollection": false
                    },
                    {
                        "text": "New York Mills, MN, USA",
                        "magicKey": "dHA9MCNsb2M9MQ==",
                        "isCollection": false
                    },
                    {
                        "text": "New York Ave NW, Washington, DC, USA",
                        "magicKey": "dHA9MCNsb2M9Mg==",
                        "isCollection": false
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = SuggestParams::new("New Y");

        let resp = client.suggest(&params).await.unwrap();

        assert_eq!(resp.suggestions.len(), 3);
        assert_eq!(resp.suggestions[0].text, "New York, NY, USA");
        assert_eq!(resp.suggestions[0].magic_key, "dHA9MCNsb2M9MA==");
        assert!(!resp.suggestions[0].is_collection);
    }

    /// Optional suggest params are forwarded when set.
    #[tokio::test]
    async fn suggest_optional_params_forwarded() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/suggest"))
            .and(query_param("location", "-77.0,38.9"))
            .and(query_param("searchExtent", "-80,35,-70,45"))
            .and(query_param("category", "City"))
            .and(query_param("countryCode", "USA"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "suggestions": []
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = SuggestParams {
            text: "New Y".to_owned(),
            location: Some("-77.0,38.9".to_owned()),
            search_extent: Some("-80,35,-70,45".to_owned()),
            category: Some("City".to_owned()),
            country_code: Some("USA".to_owned()),
            ..Default::default()
        };

        let resp = client.suggest(&params).await.unwrap();
        assert_eq!(resp.suggestions.len(), 0);
    }

    /// isCollection is correctly deserialized when true.
    #[tokio::test]
    async fn suggest_is_collection_true() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/suggest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "suggestions": [
                    {
                        "text": "Starbucks",
                        "magicKey": "abc",
                        "isCollection": true
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = SuggestParams::new("Star");

        let resp = client.suggest(&params).await.unwrap();
        assert!(resp.suggestions[0].is_collection);
    }

    /// An `ArcGIS` error from suggest surfaces as `Error::Api`.
    #[tokio::test]
    async fn suggest_api_error() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/suggest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": {
                    "code": 403,
                    "message": "Token does not have the required scope.",
                    "details": []
                }
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = SuggestParams::new("anything");

        let err = client.suggest(&params).await.unwrap_err();
        match err {
            Error::Api { code, .. } => assert_eq!(code, 403),
            other => panic!("expected Error::Api, got {other:?}"),
        }
    }

    // ── geocode_addresses ─────────────────────────────────────────────────────

    fn to_json_value<T: Serialize>(value: &T) -> JsonValue {
        let bytes = json::to_vec(value).unwrap();
        json::from_slice(&bytes).unwrap()
    }

    fn make_address_record(objectid: i64, address: &str) -> AddressRecord {
        let mut attrs = HashMap::new();
        attrs.insert("OBJECTID".to_owned(), to_json_value(&objectid));
        attrs.insert("SingleLine".to_owned(), to_json_value(&address));
        AddressRecord { attributes: attrs }
    }

    /// POST /geocodeAddresses with a form body containing `addresses` as JSON string.
    /// Returns a list of geocoded locations.
    #[tokio::test]
    async fn geocode_addresses_returns_locations() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/geocodeAddresses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "spatialReference": { "wkid": 4326, "latestWkid": 4326 },
                "locations": [
                    {
                        "address": "380 New York St, Redlands, CA 92373",
                        "location": { "x": -117.195, "y": 34.056 },
                        "score": 100.0,
                        "attributes": {},
                        "resultId": 1
                    },
                    {
                        "address": "1 World Way, Los Angeles, CA 90045",
                        "location": { "x": -118.408, "y": 33.944 },
                        "score": 98.5,
                        "attributes": {},
                        "resultId": 2
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = GeocodeAddressesParams {
            addresses: AddressRecordSet {
                records: vec![
                    make_address_record(1, "380 New York St, Redlands, CA"),
                    make_address_record(2, "1 World Way, Los Angeles, CA"),
                ],
            },
            ..Default::default()
        };

        let resp = client.geocode_addresses(&params).await.unwrap();

        assert_eq!(resp.locations.len(), 2);
        assert_eq!(resp.locations[0].result_id, 1);
        assert_eq!(
            resp.locations[0].address,
            "380 New York St, Redlands, CA 92373"
        );
        assert!((resp.locations[0].score - 100.0).abs() < 1e-6);
        assert!(resp.locations[0].location.is_some());
        assert_eq!(resp.locations[1].result_id, 2);
        assert_eq!(resp.spatial_reference.unwrap().wkid, 4326);
    }

    /// Single-record batch.
    #[tokio::test]
    async fn geocode_addresses_single_record() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/geocodeAddresses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "spatialReference": { "wkid": 4326, "latestWkid": 4326 },
                "locations": [
                    {
                        "address": "Buckingham Palace, London, UK",
                        "location": { "x": -0.1419, "y": 51.5014 },
                        "score": 95.0,
                        "attributes": {},
                        "resultId": 42
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = GeocodeAddressesParams {
            addresses: AddressRecordSet {
                records: vec![make_address_record(42, "Buckingham Palace")],
            },
            ..Default::default()
        };

        let resp = client.geocode_addresses(&params).await.unwrap();
        assert_eq!(resp.locations.len(), 1);
        assert_eq!(resp.locations[0].result_id, 42);
    }

    /// Optional geocodeAddresses params are included in the form body.
    /// We verify the endpoint is called with POST (body inspection is handled
    /// by the service; wiremock confirms the right path and method).
    #[tokio::test]
    async fn geocode_addresses_optional_params() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/geocodeAddresses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "spatialReference": { "wkid": 4326, "latestWkid": 4326 },
                "locations": []
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = GeocodeAddressesParams {
            addresses: AddressRecordSet {
                records: vec![make_address_record(1, "Test St")],
            },
            search_extent: Some("-80,35,-70,45".to_owned()),
            location_type: Some("rooftop".to_owned()),
            category: Some("Address".to_owned()),
            source_country: Some("USA".to_owned()),
            lang_code: Some("en".to_owned()),
            out_fields: Some("*".to_owned()),
            ..Default::default()
        };

        let resp = client.geocode_addresses(&params).await.unwrap();
        assert_eq!(resp.locations.len(), 0);
    }

    /// An `ArcGIS` error from `geocode_addresses` surfaces as `Error::Api`.
    #[tokio::test]
    async fn geocode_addresses_api_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/geocodeAddresses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": {
                    "code": 498,
                    "message": "Invalid token.",
                    "details": []
                }
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = GeocodeAddressesParams {
            addresses: AddressRecordSet {
                records: vec![make_address_record(1, "Test")],
            },
            ..Default::default()
        };

        let err = client.geocode_addresses(&params).await.unwrap_err();
        match err {
            Error::Api { code, .. } => assert_eq!(code, 498),
            other => panic!("expected Error::Api, got {other:?}"),
        }
    }

    // ── ClientBuilder ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn client_builder_custom_base_url_is_used() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/suggest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "suggestions": []
            })))
            .mount(&server)
            .await;

        // Build with custom base URL pointing at the mock server.
        let client = GeocoderClient::builder()
            .base_url(server.uri())
            .build(TOKEN)
            .unwrap();

        let resp = client.suggest(&SuggestParams::new("test")).await.unwrap();
        assert_eq!(resp.suggestions.len(), 0);
    }

    /// `with_token()` shortcut uses the default `ArcGIS` base URL
    /// (we cannot test the actual URL here, but we verify it constructs without error).
    #[test]
    fn with_token_constructs_without_error() {
        let client = GeocoderClient::with_token("some-token");
        assert!(client.is_ok());
    }

    // ── Send + Sync (C-SEND-SYNC) ─────────────────────────────────────────────

    #[test]
    fn geocoder_client_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<GeocoderClient>();
    }

    #[test]
    fn geocoder_client_is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<GeocoderClient>();
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Error>();
    }

    // ── Error cases ───────────────────────────────────────────────────────────

    /// HTTP-level errors (non-200 that reqwest propagates) surface as `Error::Http`.
    /// Wiremock always returns a response, so we test with a malformed JSON body
    /// to get a `Error::Json` instead.
    #[tokio::test]
    async fn malformed_json_response_is_json_error() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/suggest"))
            .respond_with(ResponseTemplate::new(200).set_body_string("this is not json"))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .suggest(&SuggestParams::new("test"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Json(_)));
    }

    /// Error code 499 (token expired) surfaces correctly.
    #[tokio::test]
    async fn token_expired_error_code() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": {
                    "code": 499,
                    "message": "Token expired.",
                    "details": []
                }
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("test".to_owned()),
            ..Default::default()
        };

        let err = client.find_address_candidates(&params).await.unwrap_err();
        match err {
            Error::Api { code, message, .. } => {
                assert_eq!(code, 499);
                assert!(message.contains("expired"));
            }
            other => panic!("expected Error::Api, got {other:?}"),
        }
    }

    // ── OAuth client-credentials flow ─────────────────────────────────────────

    use std::sync::atomic::{AtomicUsize, Ordering};

    use wiremock::matchers::body_string_contains;

    fn make_oauth_client(server_uri: &str) -> GeocoderClient {
        let creds = OAuthCredentials::new("test-client-id", "test-client-secret")
            .with_token_url(format!("{server_uri}/sharing/rest/oauth2/token"));
        GeocoderClient::builder()
            .base_url(server_uri)
            .build_oauth(creds)
            .unwrap()
    }

    /// On first request, the client posts to the OAuth token endpoint and
    /// then uses the returned `access_token` in the geocode call.
    #[tokio::test]
    async fn oauth_acquires_token_then_uses_it() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/sharing/rest/oauth2/token"))
            .and(body_string_contains("grant_type=client_credentials"))
            .and(body_string_contains("client_id=test-client-id"))
            .and(body_string_contains("client_secret=test-client-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "oauth-issued-token",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .and(header("authorization", "Bearer oauth-issued-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": []
            })))
            .mount(&server)
            .await;

        let client = make_oauth_client(&server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("anything".to_owned()),
            ..Default::default()
        };
        let resp = client.find_address_candidates(&params).await.unwrap();
        assert_eq!(resp.candidates.len(), 0);
    }

    /// Subsequent requests reuse the cached access token (only one call to
    /// the token endpoint for two geocode requests).
    #[tokio::test]
    async fn oauth_caches_token_across_requests() {
        let server = MockServer::start().await;
        let token_calls = Arc::new(AtomicUsize::new(0));

        let counter = Arc::clone(&token_calls);
        Mock::given(method("POST"))
            .and(path("/sharing/rest/oauth2/token"))
            .respond_with(move |_: &wiremock::Request| {
                counter.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": "cached-token",
                    "expires_in": 3600
                }))
            })
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .and(header("authorization", "Bearer cached-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": []
            })))
            .mount(&server)
            .await;

        let client = make_oauth_client(&server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("a".to_owned()),
            ..Default::default()
        };
        client.find_address_candidates(&params).await.unwrap();
        client.find_address_candidates(&params).await.unwrap();
        assert_eq!(token_calls.load(Ordering::SeqCst), 1);
    }

    /// After forcing the cached token to expire the client refreshes via the
    /// token endpoint on the next request.
    #[tokio::test]
    async fn oauth_refreshes_expired_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sharing/rest/oauth2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-token",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": []
            })))
            .mount(&server)
            .await;

        let client = make_oauth_client(&server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("a".to_owned()),
            ..Default::default()
        };
        client.find_address_candidates(&params).await.unwrap();
        client.force_token_refresh().await;
        client.find_address_candidates(&params).await.unwrap();

        let received = server.received_requests().await.unwrap();
        let token_calls = received
            .iter()
            .filter(|r| r.url.path() == "/sharing/rest/oauth2/token")
            .count();
        assert_eq!(token_calls, 2);
    }

    /// On `498` (invalid token) from a geocode call, an OAuth client
    /// invalidates the cached token, fetches a new one, and retries once.
    #[tokio::test]
    async fn oauth_retries_once_on_invalid_token() {
        let server = MockServer::start().await;
        let token_calls = Arc::new(AtomicUsize::new(0));

        let counter = Arc::clone(&token_calls);
        Mock::given(method("POST"))
            .and(path("/sharing/rest/oauth2/token"))
            .respond_with(move |_: &wiremock::Request| {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let access = if n == 0 { "stale-token" } else { "fresh-token" };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": access,
                    "expires_in": 3600
                }))
            })
            .mount(&server)
            .await;

        // First geocode call (with stale token) returns API error 498.
        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .and(header("authorization", "Bearer stale-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": { "code": 498, "message": "Invalid token.", "details": [] }
            })))
            .mount(&server)
            .await;

        // Retry with the refreshed token succeeds.
        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .and(header("authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": []
            })))
            .mount(&server)
            .await;

        let client = make_oauth_client(&server.uri());
        let resp = client
            .find_address_candidates(&FindAddressCandidatesParams {
                single_line: Some("a".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(resp.candidates.len(), 0);
        assert_eq!(token_calls.load(Ordering::SeqCst), 2);
    }

    /// A static-token client surfaces `498` directly without retrying (no
    /// way to refresh a user-supplied static token).
    #[tokio::test]
    async fn static_token_does_not_retry_on_invalid_token() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));

        let counter = Arc::clone(&calls);
        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .respond_with(move |_: &wiremock::Request| {
                counter.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "error": { "code": 498, "message": "Invalid token.", "details": [] }
                }))
            })
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .find_address_candidates(&FindAddressCandidatesParams {
                single_line: Some("a".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Api { code: 498, .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// An error response from the token endpoint surfaces as `Error::Api`.
    #[tokio::test]
    async fn oauth_token_endpoint_error_surfaces() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/sharing/rest/oauth2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": {
                    "code": 400,
                    "message": "Invalid client_id or client_secret.",
                    "details": []
                }
            })))
            .mount(&server)
            .await;

        let client = make_oauth_client(&server.uri());
        let params = FindAddressCandidatesParams {
            single_line: Some("a".to_owned()),
            ..Default::default()
        };
        let err = client.find_address_candidates(&params).await.unwrap_err();
        match err {
            Error::Api { code, .. } => assert_eq!(code, 400),
            other => panic!("expected Error::Api, got {other:?}"),
        }
    }

    /// `access_token()` returns the static token unchanged when the client was
    /// constructed via `with_token`.
    #[tokio::test]
    async fn static_access_token_returns_configured_value() {
        let client = GeocoderClient::with_token("static-key").unwrap();
        assert_eq!(&*client.access_token().await.unwrap(), "static-key");
    }

    /// OAuth-spec error envelope (`{"error":"invalid_client",…}`) surfaces as
    /// [`Error::Api`].
    #[tokio::test]
    async fn oauth_spec_error_envelope_surfaces() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/sharing/rest/oauth2/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_client",
                "error_description": "Client authentication failed"
            })))
            .mount(&server)
            .await;

        let client = make_oauth_client(&server.uri());
        let err = client.access_token().await.unwrap_err();
        match err {
            Error::Api { message, .. } => {
                assert!(message.contains("invalid_client"), "message: {message}");
                assert!(
                    message.contains("Client authentication failed"),
                    "message: {message}"
                );
            }
            other => panic!("expected Error::Api, got {other:?}"),
        }
    }

    /// `Debug` impl on `GeocoderClient` does not leak the static token.
    #[test]
    fn debug_redacts_static_token() {
        let client = GeocoderClient::with_token("supersecret-token").unwrap();
        let debug = format!("{client:?}");
        assert!(
            !debug.contains("supersecret-token"),
            "static token leaked via Debug: {debug}"
        );
    }

    /// `Debug` impl on `OAuthCredentials` redacts both `client_id` and `client_secret`.
    #[test]
    fn debug_redacts_oauth_credentials() {
        let creds = OAuthCredentials::new("my-client-id", "my-client-secret");
        let debug = format!("{creds:?}");
        assert!(!debug.contains("my-client-id"), "client_id leaked: {debug}");
        assert!(
            !debug.contains("my-client-secret"),
            "client_secret leaked: {debug}"
        );
    }

    /// A 200 response with both `error` and result fields present is treated
    /// as an error (the API error wins).
    #[tokio::test]
    async fn hybrid_error_envelope_is_treated_as_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/findAddressCandidates"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [],
                "error": { "code": 498, "message": "Invalid token.", "details": [] }
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .find_address_candidates(&FindAddressCandidatesParams {
                single_line: Some("x".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        match err {
            Error::Api { code, .. } => assert_eq!(code, 498),
            other => panic!("expected Error::Api, got {other:?}"),
        }
    }

    /// `AddressRecord::with_attribute` round-trips through the JSON serializer
    /// for both string and numeric values.
    #[test]
    fn address_record_with_attribute_round_trips() {
        let rec = AddressRecord::new(7)
            .with_attribute("Address", &"1 Main St")
            .with_attribute("Postal", &90210u32);
        let bytes = json::to_vec(&rec).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains("\"OBJECTID\""));
        assert!(s.contains("\"Address\""));
        assert!(s.contains("\"1 Main St\""));
        assert!(s.contains("\"Postal\""));
        assert!(s.contains("90210"));
    }

    // ── Fixture-based deserialization tests ───────────────────────────────────
    // These tests deserialize real-world-shaped ArcGIS response payloads saved
    // in `tests/fixtures/` and assert that every field maps correctly to the
    // corresponding model type.  They exercise the serde attributes (rename,
    // rename_all, default, …) in isolation from the HTTP layer.

    #[cfg(feature = "serde_json")]
    mod fixtures {
        use super::*;

        const FIND_ADDRESS_CANDIDATES: &str =
            include_str!("../tests/fixtures/find_address_candidates.json");
        const REVERSE_GEOCODE: &str = include_str!("../tests/fixtures/reverse_geocode.json");
        const SUGGEST: &str = include_str!("../tests/fixtures/suggest.json");
        const GEOCODE_ADDRESSES: &str = include_str!("../tests/fixtures/geocode_addresses.json");

        #[test]
        fn deserialize_find_address_candidates() {
            let resp: FindAddressCandidatesResponse =
                serde_json::from_str(FIND_ADDRESS_CANDIDATES).expect("fixture must deserialize");

            assert_eq!(resp.candidates.len(), 2);

            let first = &resp.candidates[0];
            assert_eq!(
                first.address,
                "1600 Pennsylvania Ave NW, Washington, DC 20500"
            );
            assert!((first.location.x - (-77.03655)).abs() < 1e-5);
            assert!((first.location.y - 38.89767).abs() < 1e-5);
            assert!((first.score - 100.0).abs() < 1e-6);
            assert!(first.attributes.contains_key("LongLabel"));
            assert!(first.attributes.contains_key("Addr_type"));

            let second = &resp.candidates[1];
            assert!(second.score < first.score);

            let sr = resp
                .spatial_reference
                .expect("spatialReference must be present");
            assert_eq!(sr.wkid, 4326);
            assert_eq!(sr.latest_wkid, Some(4326));
        }

        #[test]
        fn deserialize_reverse_geocode() {
            let resp: ReverseGeocodeResponse =
                serde_json::from_str(REVERSE_GEOCODE).expect("fixture must deserialize");

            assert!(resp.address.contains_key("LongLabel"));
            assert!(resp.address.contains_key("City"));
            assert!(resp.address.contains_key("Addr_type"));

            assert!((resp.location.x - (-77.03655)).abs() < 1e-5);
            assert!((resp.location.y - 38.89767).abs() < 1e-5);

            let sr = resp
                .location
                .spatial_reference
                .expect("spatialReference must be present on location");
            assert_eq!(sr.wkid, 4326);
        }

        #[test]
        fn deserialize_suggest() {
            let resp: SuggestResponse =
                serde_json::from_str(SUGGEST).expect("fixture must deserialize");

            assert_eq!(resp.suggestions.len(), 5);

            let first = &resp.suggestions[0];
            assert_eq!(first.text, "New York, NY, USA");
            assert!(!first.magic_key.is_empty());
            assert!(!first.is_collection);

            // last entry is a collection suggestion
            let last = resp.suggestions.last().unwrap();
            assert!(last.is_collection);
        }

        #[test]
        fn deserialize_geocode_addresses() {
            let resp: GeocodeAddressesResponse =
                serde_json::from_str(GEOCODE_ADDRESSES).expect("fixture must deserialize");

            assert_eq!(resp.locations.len(), 3);

            let first = &resp.locations[0];
            assert_eq!(first.result_id, 1);
            assert_eq!(first.address, "380 New York St, Redlands, CA 92373");
            assert!((first.score - 100.0).abs() < 1e-6);
            let loc = first.location.as_ref().expect("location must be present");
            assert!((loc.x - (-117.19555)).abs() < 1e-5);
            assert!(first.attributes.contains_key("LongLabel"));

            let second = &resp.locations[1];
            assert_eq!(second.result_id, 2);
            assert!(second.score < first.score);

            // third record is an unmatched result — location is null
            let unmatched = &resp.locations[2];
            assert_eq!(unmatched.result_id, 3);
            assert!(unmatched.location.is_none());
            assert!((unmatched.score - 0.0).abs() < f64::EPSILON);

            let sr = resp
                .spatial_reference
                .expect("spatialReference must be present");
            assert_eq!(sr.wkid, 4326);
        }
    }
}
