use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::JsonValue;

// ── Shared types ──────────────────────────────────────────────────────────────

/// A geographic point in WGS 84 (longitude/latitude).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Location {
    /// Longitude (x).
    pub x: f64,
    /// Latitude (y).
    pub y: f64,
    /// Output spatial reference, present on responses that include one (e.g. `reverseGeocode`).
    #[serde(
        rename = "spatialReference",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub spatial_reference: Option<SpatialReference>,
}

/// `ArcGIS` spatial reference descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SpatialReference {
    pub wkid: i32,
    pub latest_wkid: Option<i32>,
}

// ── findAddressCandidates ─────────────────────────────────────────────────────

/// Parameters for the `findAddressCandidates` (forward geocoding) operation.
///
/// At least one of `single_line` or `address` must be set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
pub struct FindAddressCandidatesParams {
    /// Full address as a single string (e.g. `"1600 Pennsylvania Ave NW, DC"`).
    pub single_line: Option<String>,
    /// First line of a street address (house number and street name).
    pub address: Option<String>,
    /// Second line of a street address (building name, suite, etc.).
    pub address2: Option<String>,
    /// Third line of a street address.
    pub address3: Option<String>,
    /// Neighborhood — smallest administrative subdivision of a city.
    pub neighborhood: Option<String>,
    /// City or municipality.
    pub city: Option<String>,
    /// County or department (`subregion` in the API).
    pub subregion: Option<String>,
    /// State or province.
    pub region: Option<String>,
    /// Standard postal code.
    pub postal: Option<String>,
    /// Postal code extension (e.g. US ZIP+4).
    pub postal_ext: Option<String>,
    /// ISO 3166 two- or three-character country code to restrict results.
    pub country_code: Option<String>,
    /// Place or address type filter, e.g. `"POI"`, `"Address"`, `"Postal"`.
    pub category: Option<String>,
    /// Comma-separated output field list, or `"*"` to return all fields.
    pub out_fields: Option<String>,
    /// Well-known ID (WKID) for the output spatial reference (default `4326`).
    pub out_sr: Option<i32>,
    /// Maximum candidates to return (up to 50; omit for the service default).
    pub max_locations: Option<u32>,
    /// Bounding box to restrict results, e.g. `"xmin,ymin,xmax,ymax"` (WGS 84).
    pub search_extent: Option<String>,
    /// Bias point for nearby results, e.g. `"lon,lat"` (WGS 84).
    pub location: Option<String>,
    /// BCP 47 language code for returned text (e.g. `"en"`).
    pub lang_code: Option<String>,
    /// `"rooftop"` or `"street"` — geometry of PointAddress/Subaddress matches.
    pub location_type: Option<String>,
    /// Limit candidates to this country (three-character code); similar to
    /// `country_code` but searched against the source data rather than the input.
    pub source_country: Option<String>,
    /// Control which city or street name variant appears in output fields.
    /// Valid values: `"postalCity"`, `"localCity"`, `"matchedCity"`,
    /// `"primaryStreet"`, `"matchedStreet"`.
    pub preferred_label_values: Option<String>,
    /// Return a match when the house number is outside the street's range.
    /// Defaults to `true`.
    pub match_out_of_range: Option<bool>,
    /// `true` when persisting results (required by `ToS`; triggers stored-geocode billing).
    pub for_storage: Option<bool>,
    /// Fuzzy-match adjacent postal/admin zones. Defaults to `true`.
    pub comprehensive_zone_match: Option<bool>,
    /// Return detailed per-component match information in the `MatchNarrative` field.
    pub return_match_narrative: Option<bool>,
    /// Include primary street/place names in the `MatchID` value.
    pub return_primary_match_id: Option<bool>,
    /// Exclude intersection type from results, e.g. `"virtual"`.
    pub exclude_intersection_type: Option<String>,
    /// Search by an opaque match ID returned by a prior geocode.
    pub match_id: Option<String>,
    /// Return a collection of related features (`"PointAddress"`, `"Subaddress"`,
    /// or `"POI"`) associated with the geocoded result.
    pub search_within: Option<String>,
    /// First result index for `search_within` pagination (default `1`).
    pub start: Option<u32>,
    /// Page size for `search_within` pagination (1–50; default `50`).
    pub num: Option<u32>,
    /// Magic key returned by a prior `suggest` call to resolve a specific suggestion.
    /// Must be combined with `single_line`.
    pub magic_key: Option<String>,
}

/// Response from the `findAddressCandidates` operation.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FindAddressCandidatesResponse {
    pub candidates: Vec<AddressCandidate>,
    #[serde(default)]
    pub spatial_reference: Option<SpatialReference>,
}

/// A single address candidate returned by `findAddressCandidates`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct AddressCandidate {
    pub address: String,
    pub location: Location,
    pub score: f64,
    #[serde(default)]
    pub attributes: HashMap<String, JsonValue>,
}

// ── reverseGeocode ────────────────────────────────────────────────────────────

/// Parameters for the `reverseGeocode` operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
pub struct ReverseGeocodeParams {
    /// The point to reverse-geocode as `"lon,lat"` (WGS 84) or a JSON point
    /// object (`{"x":...,"y":...}` with optional `spatialReference`).
    pub location: String,
    /// Restrict match types, e.g. `"StreetAddress"`, `"PointAddress"`,
    /// `"StreetInt"`, `"POI"`, `"Locality"`. Multiple values are comma-separated.
    /// Using a single value extends the search tolerance to 500 m.
    pub feature_types: Option<String>,
    /// `"rooftop"` (default) or `"street"` — geometry of PointAddress/Subaddress matches.
    pub location_type: Option<String>,
    /// **Deprecated** — use `feature_types = "StreetInt"` instead.
    /// Return the nearest street intersection rather than an address.
    pub return_intersection: Option<bool>,
    /// BCP 47 language code for returned text.
    pub lang_code: Option<String>,
    /// `true` when persisting results (required by `ToS`; triggers stored-geocode billing).
    pub for_storage: Option<bool>,
    /// Well-known ID (WKID) for the output spatial reference (default `4326`).
    pub out_sr: Option<i32>,
    /// Control which city name variant appears in output fields.
    /// Valid values: `"postalCity"`, `"localCity"`.
    pub preferred_label_values: Option<String>,
    /// Comma-separated output field list, or `"*"` for all fields.
    /// All fields are returned by default.
    pub out_fields: Option<String>,
    /// If `true`, the input `location` coordinates are returned in the `X`/`Y`
    /// output fields rather than the geocoded location coordinates.
    pub return_input_location: Option<bool>,
}

impl ReverseGeocodeParams {
    /// Creates a new [`ReverseGeocodeParams`] with the required location string.
    ///
    /// # Example
    ///
    /// ```
    /// let params = arcgis_geocoder::ReverseGeocodeParams::new("-77.036556,38.897663");
    /// assert_eq!(params.location, "-77.036556,38.897663");
    /// ```
    #[must_use]
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            ..Default::default()
        }
    }
}

/// Response from the `reverseGeocode` operation.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ReverseGeocodeResponse {
    /// Address attributes keyed by field name (e.g. `LongLabel`, `City`, `Region`).
    pub address: HashMap<String, JsonValue>,
    pub location: Location,
}

// ── suggest ───────────────────────────────────────────────────────────────────

/// Parameters for the `suggest` (autosuggest) operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
pub struct SuggestParams {
    /// Partial address or place name (typically 2–3+ characters).
    pub text: String,
    /// Bias point, e.g. `"lon,lat"` (WGS 84). Prioritises nearby results within
    /// a 50 km radius but does not filter out distant ones.
    pub location: Option<String>,
    /// Bounding box to restrict suggestions to a specific area,
    /// e.g. `"xmin,ymin,xmax,ymax"` (WGS 84).
    pub search_extent: Option<String>,
    /// Place or address type filter, e.g. `"Address"`, `"POI"`, `"Amusement Park"`.
    pub category: Option<String>,
    /// ISO 3166 two- or three-character country code to restrict suggestions
    /// to a single country.
    pub country_code: Option<String>,
    /// Maximum suggestions to return (1–15; default `5`).
    pub max_suggestions: Option<u32>,
    /// Restrict suggestions to one or more countries (comma-separated three-character
    /// codes). Takes priority over `country_code` when both are present.
    pub source_country: Option<String>,
    /// Control which city name variant appears in suggestion labels.
    /// Valid values: `"postalCity"`, `"localCity"`.
    pub preferred_label_values: Option<String>,
    /// Include collection suggestions (e.g. `"Coffee Shop"` — a category rather
    /// than a specific place). Defaults to `true`.
    pub return_collections: Option<bool>,
    /// Allow `PointAddress` suggestions when only a partial house number is entered.
    /// Only supported for address formats where the house number follows the street name.
    pub partial_house_number: Option<bool>,
    /// Allow Subaddress suggestions when only a partial subunit value is entered.
    pub partial_subaddress: Option<bool>,
    /// Include a count or range of Subaddresses in parentheses for `PointAddress`
    /// suggestions, e.g. `"865 S Figueroa St (Suite 10-3500), Los Angeles, CA"`.
    pub subaddress_summary: Option<bool>,
    /// Return the list of Subaddresses belonging to a `PointAddress` after the
    /// base address suggestion.
    pub subaddress_after_base_address: Option<bool>,
}

impl SuggestParams {
    /// Creates a new [`SuggestParams`] with the required partial text query.
    ///
    /// # Example
    ///
    /// ```
    /// let params = arcgis_geocoder::SuggestParams::new("1600 Penn");
    /// assert_eq!(params.text, "1600 Penn");
    /// ```
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

/// Response from the `suggest` operation.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SuggestResponse {
    pub suggestions: Vec<Suggestion>,
}

/// A single suggestion returned by the `suggest` operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Suggestion {
    pub text: String,
    /// Whether the suggestion represents a collection of places rather than one.
    pub is_collection: bool,
    /// Opaque key to pass to `findAddressCandidates` to resolve this suggestion.
    pub magic_key: String,
}

// ── geocodeAddresses ──────────────────────────────────────────────────────────

/// Parameters for the `geocodeAddresses` (batch geocoding) operation.
///
/// Always billed as stored geocodes. The token must have the
/// `premium:user:geocode:stored` privilege.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GeocodeAddressesParams {
    /// The addresses to geocode. Each record must include an `OBJECTID` attribute
    /// plus `SingleLine` (single-field) or multi-field address components.
    pub addresses: AddressRecordSet,
    /// Apply a single source country to every record in the batch
    /// (three-character country code). Takes priority over per-record
    /// `CountryCode` attributes when both are present.
    pub source_country: Option<String>,
    /// Bounding box to restrict results, e.g. `"xmin,ymin,xmax,ymax"` (WGS 84).
    pub search_extent: Option<String>,
    /// `"rooftop"` (default) or `"street"` — geometry of PointAddress/Subaddress matches.
    pub location_type: Option<String>,
    /// Place or address type filter, e.g. `"Address"`, `"PointAddress,StreetAddress"`.
    pub category: Option<String>,
    /// BCP 47 language code for returned text.
    pub lang_code: Option<String>,
    /// Comma-separated output field list, or `"*"` / blank for all fields,
    /// or `"none"` for the minimum set.
    pub out_fields: Option<String>,
    /// Well-known ID (WKID) for the output spatial reference (default `4326`).
    pub out_sr: Option<i32>,
    /// Return a match when the house number is outside the street's range.
    /// Defaults to `true`.
    pub match_out_of_range: Option<bool>,
    /// Control which city or street name variant appears in output fields.
    /// Valid values: `"postalCity"`, `"localCity"`, `"matchedCity"`,
    /// `"primaryStreet"`, `"matchedStreet"`.
    pub preferred_label_values: Option<String>,
    /// Fuzzy-match adjacent postal/admin zones. Defaults to `true`.
    pub comprehensive_zone_match: Option<bool>,
    /// Return detailed per-component match information in the `MatchNarrative`
    /// output field (must also be included in `out_fields`).
    pub return_match_narrative: Option<bool>,
    /// Exclude intersection type from results, e.g. `"virtual"`.
    pub exclude_intersection_type: Option<String>,
}

/// The input address collection for `geocodeAddresses`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AddressRecordSet {
    pub records: Vec<AddressRecord>,
}

impl AddressRecordSet {
    /// Creates an empty record set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a record set from an iterator of [`AddressRecord`]s.
    pub fn from_records<I: IntoIterator<Item = AddressRecord>>(records: I) -> Self {
        Self {
            records: records.into_iter().collect(),
        }
    }

    /// Appends a record to the set.
    pub fn push(&mut self, record: AddressRecord) {
        self.records.push(record);
    }
}

/// A single address record within an [`AddressRecordSet`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AddressRecord {
    /// Must contain at least `"OBJECTID"` and `"SingleLine"` (or multi-field components).
    pub attributes: HashMap<String, JsonValue>,
}

impl AddressRecord {
    /// Creates a new record with the given `OBJECTID` and a single-line address.
    ///
    /// This populates the `OBJECTID` and `SingleLine` attributes, which is the
    /// most common batch-geocoding shape.
    ///
    /// # Example
    ///
    /// ```
    /// use arcgis_geocoder::AddressRecord;
    /// let rec = AddressRecord::single_line(1, "380 New York St, Redlands, CA");
    /// assert!(rec.attributes.contains_key("OBJECTID"));
    /// assert!(rec.attributes.contains_key("SingleLine"));
    /// ```
    #[must_use]
    pub fn single_line(objectid: i64, address: impl Into<String>) -> Self {
        let mut attributes = HashMap::with_capacity(2);
        attributes.insert("OBJECTID".to_owned(), crate::json::to_value(&objectid));
        attributes.insert(
            "SingleLine".to_owned(),
            crate::json::to_value(&address.into()),
        );
        Self { attributes }
    }

    /// Creates a new record containing only the `OBJECTID` attribute.
    /// Use [`with_attribute`](Self::with_attribute) to add address components.
    #[must_use]
    pub fn new(objectid: i64) -> Self {
        let mut attributes = HashMap::with_capacity(1);
        attributes.insert("OBJECTID".to_owned(), crate::json::to_value(&objectid));
        Self { attributes }
    }

    /// Builder-style: inserts an attribute and returns `self`.
    ///
    /// The `value` is serialized into the active backend's JSON value type.
    #[must_use]
    pub fn with_attribute<V: Serialize>(mut self, key: impl Into<String>, value: &V) -> Self {
        self.attributes
            .insert(key.into(), crate::json::to_value(value));
        self
    }
}

/// Response from the `geocodeAddresses` operation.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GeocodeAddressesResponse {
    #[serde(default)]
    pub spatial_reference: Option<SpatialReference>,
    pub locations: Vec<GeocodeLocation>,
}

/// A geocoded location within a [`GeocodeAddressesResponse`].
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GeocodeLocation {
    pub address: String,
    pub location: Option<Location>,
    pub score: f64,
    #[serde(default)]
    pub attributes: HashMap<String, JsonValue>,
    /// Matches the `OBJECTID` of the corresponding input [`AddressRecord`].
    pub result_id: i64,
}
