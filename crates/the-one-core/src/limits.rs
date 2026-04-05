use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurableLimits {
    pub max_tool_suggestions: usize,
    pub max_search_hits: usize,
    pub max_raw_section_bytes: usize,
    pub max_enabled_families: usize,
    pub max_doc_size_bytes: usize,
    pub max_managed_docs: usize,
    pub max_embedding_batch_size: usize,
    pub max_chunk_tokens: usize,
    pub max_nano_timeout_ms: u64,
    pub max_nano_retries: u8,
    pub max_nano_providers: usize,
    pub search_score_threshold: f32,
    /// Maximum image file size in bytes (default 10MB).
    #[serde(default = "default_max_image_size_bytes")]
    pub max_image_size_bytes: usize,
    /// Maximum number of images per project.
    #[serde(default = "default_max_images_per_project")]
    pub max_images_per_project: usize,
    /// Maximum number of image search results.
    #[serde(default = "default_max_image_search_hits")]
    pub max_image_search_hits: usize,
    /// Minimum score threshold for image search results.
    #[serde(default = "default_image_search_score_threshold")]
    pub image_search_score_threshold: f32,
}

fn default_max_image_size_bytes() -> usize {
    10_485_760
}
fn default_max_images_per_project() -> usize {
    500
}
fn default_max_image_search_hits() -> usize {
    5
}
fn default_image_search_score_threshold() -> f32 {
    0.25
}

impl Default for ConfigurableLimits {
    fn default() -> Self {
        Self {
            max_tool_suggestions: 5,
            max_search_hits: 5,
            max_raw_section_bytes: 24 * 1024,
            max_enabled_families: 12,
            max_doc_size_bytes: 100 * 1024,
            max_managed_docs: 500,
            max_embedding_batch_size: 64,
            max_chunk_tokens: 512,
            max_nano_timeout_ms: 2000,
            max_nano_retries: 3,
            max_nano_providers: 5,
            search_score_threshold: 0.3,
            max_image_size_bytes: 10_485_760, // 10MB
            max_images_per_project: 500,
            max_image_search_hits: 5,
            image_search_score_threshold: 0.25,
        }
    }
}

fn clamp_usize(name: &str, value: usize, min: usize, max: usize) -> usize {
    if value < min {
        tracing::warn!("{name} value {value} below minimum {min}, clamping to {min}");
        min
    } else if value > max {
        tracing::warn!("{name} value {value} above maximum {max}, clamping to {max}");
        max
    } else {
        value
    }
}

fn clamp_u64(name: &str, value: u64, min: u64, max: u64) -> u64 {
    if value < min {
        tracing::warn!("{name} value {value} below minimum {min}, clamping to {min}");
        min
    } else if value > max {
        tracing::warn!("{name} value {value} above maximum {max}, clamping to {max}");
        max
    } else {
        value
    }
}

fn clamp_u8(name: &str, value: u8, min: u8, max: u8) -> u8 {
    if value < min {
        tracing::warn!("{name} value {value} below minimum {min}, clamping to {min}");
        min
    } else if value > max {
        tracing::warn!("{name} value {value} above maximum {max}, clamping to {max}");
        max
    } else {
        value
    }
}

fn clamp_f32(name: &str, value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        tracing::warn!("{name} value {value} below minimum {min}, clamping to {min}");
        min
    } else if value > max {
        tracing::warn!("{name} value {value} above maximum {max}, clamping to {max}");
        max
    } else {
        value
    }
}

impl ConfigurableLimits {
    pub fn validated(mut self) -> Self {
        self.max_tool_suggestions =
            clamp_usize("max_tool_suggestions", self.max_tool_suggestions, 1, 50);
        self.max_search_hits = clamp_usize("max_search_hits", self.max_search_hits, 1, 100);
        self.max_raw_section_bytes = clamp_usize(
            "max_raw_section_bytes",
            self.max_raw_section_bytes,
            1024,
            1_048_576,
        );
        self.max_enabled_families =
            clamp_usize("max_enabled_families", self.max_enabled_families, 1, 100);
        self.max_doc_size_bytes = clamp_usize(
            "max_doc_size_bytes",
            self.max_doc_size_bytes,
            1024,
            10_485_760,
        );
        self.max_managed_docs = clamp_usize("max_managed_docs", self.max_managed_docs, 10, 10_000);
        self.max_embedding_batch_size = clamp_usize(
            "max_embedding_batch_size",
            self.max_embedding_batch_size,
            1,
            256,
        );
        self.max_chunk_tokens = clamp_usize("max_chunk_tokens", self.max_chunk_tokens, 64, 2_048);
        self.max_nano_timeout_ms =
            clamp_u64("max_nano_timeout_ms", self.max_nano_timeout_ms, 100, 10_000);
        self.max_nano_retries = clamp_u8("max_nano_retries", self.max_nano_retries, 0, 10);
        self.max_nano_providers = clamp_usize("max_nano_providers", self.max_nano_providers, 1, 10);
        self.search_score_threshold = clamp_f32(
            "search_score_threshold",
            self.search_score_threshold,
            0.0,
            1.0,
        );
        self.max_image_size_bytes = clamp_usize(
            "max_image_size_bytes",
            self.max_image_size_bytes,
            102_400,     // 100 KB min
            104_857_600, // 100 MB max
        );
        self.max_images_per_project = clamp_usize(
            "max_images_per_project",
            self.max_images_per_project,
            10,
            10_000,
        );
        self.max_image_search_hits =
            clamp_usize("max_image_search_hits", self.max_image_search_hits, 1, 50);
        self.image_search_score_threshold = clamp_f32(
            "image_search_score_threshold",
            self.image_search_score_threshold,
            0.0,
            1.0,
        );
        self
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigurableLimits;

    #[test]
    fn test_defaults_are_within_bounds() {
        let defaults = ConfigurableLimits::default();
        let validated = defaults.clone().validated();

        assert_eq!(
            validated.max_tool_suggestions,
            defaults.max_tool_suggestions
        );
        assert_eq!(validated.max_search_hits, defaults.max_search_hits);
        assert_eq!(
            validated.max_raw_section_bytes,
            defaults.max_raw_section_bytes
        );
        assert_eq!(
            validated.max_enabled_families,
            defaults.max_enabled_families
        );
        assert_eq!(validated.max_doc_size_bytes, defaults.max_doc_size_bytes);
        assert_eq!(validated.max_managed_docs, defaults.max_managed_docs);
        assert_eq!(
            validated.max_embedding_batch_size,
            defaults.max_embedding_batch_size
        );
        assert_eq!(validated.max_chunk_tokens, defaults.max_chunk_tokens);
        assert_eq!(validated.max_nano_timeout_ms, defaults.max_nano_timeout_ms);
        assert_eq!(validated.max_nano_retries, defaults.max_nano_retries);
        assert_eq!(validated.max_nano_providers, defaults.max_nano_providers);
        assert!(
            (validated.search_score_threshold - defaults.search_score_threshold).abs()
                < f32::EPSILON
        );
        assert_eq!(
            validated.max_image_size_bytes,
            defaults.max_image_size_bytes
        );
        assert_eq!(
            validated.max_images_per_project,
            defaults.max_images_per_project
        );
        assert_eq!(
            validated.max_image_search_hits,
            defaults.max_image_search_hits
        );
        assert!(
            (validated.image_search_score_threshold - defaults.image_search_score_threshold).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn test_out_of_bounds_values_are_clamped() {
        let too_low = ConfigurableLimits {
            max_tool_suggestions: 0,
            max_search_hits: 0,
            max_raw_section_bytes: 0,
            max_enabled_families: 0,
            max_doc_size_bytes: 0,
            max_managed_docs: 0,
            max_embedding_batch_size: 0,
            max_chunk_tokens: 0,
            max_nano_timeout_ms: 0,
            max_nano_retries: 0,
            max_nano_providers: 0,
            search_score_threshold: -1.0,
            max_image_size_bytes: 0,
            max_images_per_project: 0,
            max_image_search_hits: 0,
            image_search_score_threshold: -1.0,
        };
        let clamped = too_low.validated();

        assert_eq!(clamped.max_tool_suggestions, 1);
        assert_eq!(clamped.max_search_hits, 1);
        assert_eq!(clamped.max_raw_section_bytes, 1024);
        assert_eq!(clamped.max_enabled_families, 1);
        assert_eq!(clamped.max_doc_size_bytes, 1024);
        assert_eq!(clamped.max_managed_docs, 10);
        assert_eq!(clamped.max_embedding_batch_size, 1);
        assert_eq!(clamped.max_chunk_tokens, 64);
        assert_eq!(clamped.max_nano_timeout_ms, 100);
        assert_eq!(clamped.max_nano_retries, 0);
        assert_eq!(clamped.max_nano_providers, 1);
        assert!((clamped.search_score_threshold - 0.0).abs() < f32::EPSILON);
        assert_eq!(clamped.max_image_size_bytes, 102_400);
        assert_eq!(clamped.max_images_per_project, 10);
        assert_eq!(clamped.max_image_search_hits, 1);
        assert!((clamped.image_search_score_threshold - 0.0).abs() < f32::EPSILON);

        let too_high = ConfigurableLimits {
            max_tool_suggestions: 999,
            max_search_hits: 999,
            max_raw_section_bytes: 99_999_999,
            max_enabled_families: 999,
            max_doc_size_bytes: 99_999_999,
            max_managed_docs: 99_999,
            max_embedding_batch_size: 999,
            max_chunk_tokens: 99_999,
            max_nano_timeout_ms: 99_999,
            max_nano_retries: 99,
            max_nano_providers: 99,
            search_score_threshold: 5.0,
            max_image_size_bytes: 999_999_999,
            max_images_per_project: 999_999,
            max_image_search_hits: 999,
            image_search_score_threshold: 5.0,
        };
        let clamped = too_high.validated();

        assert_eq!(clamped.max_tool_suggestions, 50);
        assert_eq!(clamped.max_search_hits, 100);
        assert_eq!(clamped.max_raw_section_bytes, 1_048_576);
        assert_eq!(clamped.max_enabled_families, 100);
        assert_eq!(clamped.max_doc_size_bytes, 10_485_760);
        assert_eq!(clamped.max_managed_docs, 10_000);
        assert_eq!(clamped.max_embedding_batch_size, 256);
        assert_eq!(clamped.max_chunk_tokens, 2_048);
        assert_eq!(clamped.max_nano_timeout_ms, 10_000);
        assert_eq!(clamped.max_nano_retries, 10);
        assert_eq!(clamped.max_nano_providers, 10);
        assert!((clamped.search_score_threshold - 1.0).abs() < f32::EPSILON);
        assert_eq!(clamped.max_image_size_bytes, 104_857_600);
        assert_eq!(clamped.max_images_per_project, 10_000);
        assert_eq!(clamped.max_image_search_hits, 50);
        assert!((clamped.image_search_score_threshold - 1.0).abs() < f32::EPSILON);
    }
}
