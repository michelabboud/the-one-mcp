use crate::RequestIntent;

/// A single OpenAI-compatible LLM provider for request classification.
pub struct OpenAiCompatibleProvider {
    pub name: String,
    pub base_url: String,
    pub model: String,
    client: reqwest::Client,
    timeout: std::time::Duration,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        name: &str,
        base_url: &str,
        model: &str,
        api_key: Option<&str>,
        timeout_ms: u64,
    ) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(key) = api_key {
            if let Ok(v) = format!("Bearer {key}").parse() {
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .expect("reqwest client build");
        Self {
            name: name.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            client,
            timeout: std::time::Duration::from_millis(timeout_ms),
        }
    }

    /// Classify a request by sending a short prompt to the LLM.
    pub async fn classify(&self, request: &str) -> Result<RequestIntent, String> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": format!(
                    "Classify this request into exactly one category.\n\
                     Respond with ONLY one word: search_docs, run_tool, configure_system, or unknown.\n\n\
                     Request: \"{}\"", request
                )
            }],
            "max_tokens": 10,
            "temperature": 0.0,
        });

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("provider {} request failed: {e}", self.name))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("provider {} error {status}: {text}", self.name));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("provider {} parse failed: {e}", self.name))?;

        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("unknown")
            .trim()
            .to_ascii_lowercase();

        Ok(match content.as_str() {
            "search_docs" => RequestIntent::SearchDocs,
            "run_tool" => RequestIntent::RunTool,
            "configure_system" => RequestIntent::ConfigureSystem,
            _ => RequestIntent::Unknown,
        })
    }

    /// TCP connect check -- just verify the host is reachable.
    pub async fn tcp_check(&self) -> bool {
        if let Ok(url) = reqwest::Url::parse(&self.base_url) {
            let host = url.host_str().unwrap_or("localhost");
            let port = url.port_or_known_default().unwrap_or(80);
            let addr = format!("{host}:{port}");
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                tokio::net::TcpStream::connect(&addr),
            )
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false)
        } else {
            false
        }
    }

    /// Returns the configured timeout duration.
    pub fn timeout(&self) -> std::time::Duration {
        self.timeout
    }
}

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
