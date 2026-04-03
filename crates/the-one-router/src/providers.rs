use crate::RequestIntent;

pub trait NanoProvider {
    fn name(&self) -> &'static str;
    fn classify(&self, request: &str) -> Result<RequestIntent, String>;
}

#[derive(Debug, Clone)]
pub struct ApiNanoProvider {
    model: String,
}

#[derive(Debug, Clone, Default)]
pub struct OllamaNanoProvider;

#[derive(Debug, Clone, Default)]
pub struct LmStudioNanoProvider;

impl ApiNanoProvider {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl NanoProvider for ApiNanoProvider {
    fn name(&self) -> &'static str {
        "api-nano"
    }

    fn classify(&self, request: &str) -> Result<RequestIntent, String> {
        if request.to_lowercase().contains("nano-fail") {
            return Err("api provider classification failed".to_string());
        }
        Ok(classify_keywords(request))
    }
}

impl NanoProvider for OllamaNanoProvider {
    fn name(&self) -> &'static str {
        "ollama-nano"
    }

    fn classify(&self, request: &str) -> Result<RequestIntent, String> {
        if request.to_lowercase().contains("nano-fail") {
            return Err("ollama provider classification failed".to_string());
        }
        Ok(classify_keywords(request))
    }
}

impl NanoProvider for LmStudioNanoProvider {
    fn name(&self) -> &'static str {
        "lmstudio-nano"
    }

    fn classify(&self, request: &str) -> Result<RequestIntent, String> {
        if request.to_lowercase().contains("nano-fail") {
            return Err("lmstudio provider classification failed".to_string());
        }
        Ok(classify_keywords(request))
    }
}

fn classify_keywords(request: &str) -> RequestIntent {
    let lower = request.to_lowercase();
    if lower.contains("search") || lower.contains("docs") {
        return RequestIntent::SearchDocs;
    }
    if lower.contains("run") || lower.contains("execute") {
        return RequestIntent::RunTool;
    }
    if lower.contains("config") || lower.contains("setup") {
        return RequestIntent::ConfigureSystem;
    }
    RequestIntent::Unknown
}

#[cfg(test)]
mod tests {
    use super::{ApiNanoProvider, NanoProvider, OllamaNanoProvider};
    use crate::RequestIntent;

    #[test]
    fn test_provider_keyword_classification() {
        let provider = OllamaNanoProvider;
        assert_eq!(
            provider
                .classify("search docs")
                .expect("classify should work"),
            RequestIntent::SearchDocs
        );
        assert_eq!(
            provider
                .classify("run migration")
                .expect("classify should work"),
            RequestIntent::RunTool
        );
    }

    #[test]
    fn test_api_provider_model_is_exposed() {
        let provider = ApiNanoProvider::new("gpt-nano");
        assert_eq!(provider.model(), "gpt-nano");
    }
}
