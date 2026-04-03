#[cfg(feature = "embed-swagger")]
const EMBEDDED_SWAGGER_JSON: &str =
    include_str!("../../../schemas/mcp/v1beta/openapi.swagger.json");

pub fn embedded_swagger_json() -> Option<&'static str> {
    #[cfg(feature = "embed-swagger")]
    {
        Some(EMBEDDED_SWAGGER_JSON)
    }

    #[cfg(not(feature = "embed-swagger"))]
    {
        None
    }
}

pub const fn swagger_embedded_enabled() -> bool {
    cfg!(feature = "embed-swagger")
}

#[cfg(test)]
mod tests {
    use super::{embedded_swagger_json, swagger_embedded_enabled};

    #[test]
    fn test_swagger_embed_flag_matches_payload_presence() {
        assert_eq!(
            swagger_embedded_enabled(),
            embedded_swagger_json().is_some()
        );
    }

    #[cfg(feature = "embed-swagger")]
    #[test]
    fn test_embedded_swagger_contains_openapi_marker() {
        let payload = embedded_swagger_json().expect("swagger payload should exist");
        assert!(payload.contains("\"openapi\""));
    }
}
