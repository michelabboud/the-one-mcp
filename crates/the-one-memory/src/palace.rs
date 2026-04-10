use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PalaceMetadata {
    pub wing: String,
    pub hall: Option<String>,
    pub room: Option<String>,
}

impl PalaceMetadata {
    pub fn new(wing: &str, hall: Option<String>, room: Option<String>) -> Self {
        Self {
            wing: wing.to_string(),
            hall,
            room,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_palace_metadata_from_project_and_tags() {
        let meta = PalaceMetadata::new(
            "proj-auth",
            Some("hall_facts".to_string()),
            Some("auth-migration".to_string()),
        );

        assert_eq!(meta.wing, "proj-auth");
        assert_eq!(meta.hall.as_deref(), Some("hall_facts"));
        assert_eq!(meta.room.as_deref(), Some("auth-migration"));
    }
}
