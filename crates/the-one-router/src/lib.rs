pub mod health;
pub mod provider_pool;
pub mod providers;

use std::time::Instant;

use serde::{Deserialize, Serialize};
use the_one_core::contracts::{RouteDecision, RouteType};

use crate::providers::NanoProvider;

const MAX_NANO_TIMEOUT_MS: u64 = 2_000;
const MAX_NANO_RETRIES: u8 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RequestIntent {
    SearchDocs,
    RunTool,
    ConfigureSystem,
    Unknown,
}

pub trait NanoClassifier {
    fn classify(&self, request: &str) -> RequestIntent;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NanoBudget {
    pub timeout_ms: u64,
    pub retries: u8,
}

impl Default for NanoBudget {
    fn default() -> Self {
        Self {
            timeout_ms: 300,
            retries: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteTelemetry {
    pub provider_path: String,
    pub confidence_percent: u8,
    pub latency_ms: u64,
    pub used_fallback: bool,
    pub attempts: u8,
    pub timeout_ms_bound: u64,
    pub retries_bound: u8,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutedDecision {
    pub decision: RouteDecision,
    pub telemetry: RouteTelemetry,
}

#[derive(Debug, Clone)]
pub struct Router {
    nano_enabled: bool,
}

impl Router {
    pub fn new(nano_enabled: bool) -> Self {
        Self { nano_enabled }
    }

    pub fn route_rules_only(&self, request: &str) -> RouteDecision {
        let request_lower = request.to_lowercase();
        if request_lower.contains("search") || request_lower.contains("docs") {
            return RouteDecision {
                route: RouteType::Retrieval,
                requires_memory_search: true,
                requires_approval: false,
                rationale: "request indicates retrieval/doc lookup".to_string(),
            };
        }

        if request_lower.contains("run") || request_lower.contains("execute") {
            return RouteDecision {
                route: RouteType::ToolExecution,
                requires_memory_search: false,
                requires_approval: true,
                rationale: "request indicates tool execution".to_string(),
            };
        }

        default_route_decision()
    }

    pub fn route_with_optional_nano(
        &self,
        request: &str,
        nano: Option<&dyn NanoClassifier>,
    ) -> RouteDecision {
        if !self.nano_enabled {
            return self.route_rules_only(request);
        }

        if let Some(classifier) = nano {
            let intent = classifier.classify(request);
            let decision = match intent {
                RequestIntent::SearchDocs => RouteDecision {
                    route: RouteType::RuleWithNano,
                    requires_memory_search: true,
                    requires_approval: false,
                    rationale: "nano classifier selected search path".to_string(),
                },
                RequestIntent::RunTool => RouteDecision {
                    route: RouteType::RuleWithNano,
                    requires_memory_search: false,
                    requires_approval: true,
                    rationale: "nano classifier selected execution path".to_string(),
                },
                RequestIntent::ConfigureSystem | RequestIntent::Unknown => {
                    self.route_rules_only(request)
                }
            };
            return decision;
        }

        self.route_rules_only(request)
    }

    pub fn route_with_provider(
        &self,
        request: &str,
        provider: Option<&dyn NanoProvider>,
    ) -> RouteDecision {
        self.route_with_provider_budget(request, provider, NanoBudget::default())
            .decision
    }

    pub fn route_with_provider_budget(
        &self,
        request: &str,
        provider: Option<&dyn NanoProvider>,
        budget: NanoBudget,
    ) -> RoutedDecision {
        let start = Instant::now();
        let bounded_timeout_ms = budget.timeout_ms.min(MAX_NANO_TIMEOUT_MS);
        let bounded_retries = budget.retries.min(MAX_NANO_RETRIES);
        if !self.nano_enabled {
            let decision = self.route_rules_only(request);
            return RoutedDecision {
                decision,
                telemetry: RouteTelemetry {
                    provider_path: "rules-only".to_string(),
                    confidence_percent: 100,
                    latency_ms: start.elapsed().as_millis() as u64,
                    used_fallback: false,
                    attempts: 0,
                    timeout_ms_bound: bounded_timeout_ms,
                    retries_bound: bounded_retries,
                    last_error: None,
                },
            };
        }

        if let Some(provider) = provider {
            let mut attempts = 0u8;
            let mut last_error = None;
            loop {
                attempts = attempts.saturating_add(1);
                let attempt_result = provider.classify(request);
                if let Ok(intent) = attempt_result {
                    let decision = match intent {
                        RequestIntent::SearchDocs => RouteDecision {
                            route: RouteType::RuleWithNano,
                            requires_memory_search: true,
                            requires_approval: false,
                            rationale: format!("{} provider selected search path", provider.name()),
                        },
                        RequestIntent::RunTool => RouteDecision {
                            route: RouteType::RuleWithNano,
                            requires_memory_search: false,
                            requires_approval: true,
                            rationale: format!(
                                "{} provider selected run-tool path",
                                provider.name()
                            ),
                        },
                        RequestIntent::ConfigureSystem | RequestIntent::Unknown => {
                            self.route_rules_only(request)
                        }
                    };
                    return RoutedDecision {
                        decision,
                        telemetry: RouteTelemetry {
                            provider_path: provider.name().to_string(),
                            confidence_percent: 85,
                            latency_ms: start.elapsed().as_millis() as u64,
                            used_fallback: false,
                            attempts,
                            timeout_ms_bound: bounded_timeout_ms,
                            retries_bound: bounded_retries,
                            last_error,
                        },
                    };
                }

                if let Err(err) = attempt_result {
                    last_error = Some(err);
                }

                let deadline_exceeded = start.elapsed().as_millis() as u64 >= bounded_timeout_ms;
                let retries_exceeded = attempts > bounded_retries;
                if deadline_exceeded || retries_exceeded {
                    let decision = self.route_rules_only(request);
                    return RoutedDecision {
                        decision,
                        telemetry: RouteTelemetry {
                            provider_path: provider.name().to_string(),
                            confidence_percent: 100,
                            latency_ms: start.elapsed().as_millis() as u64,
                            used_fallback: true,
                            attempts,
                            timeout_ms_bound: bounded_timeout_ms,
                            retries_bound: bounded_retries,
                            last_error,
                        },
                    };
                }
            }
        }

        let decision = self.route_rules_only(request);
        RoutedDecision {
            decision,
            telemetry: RouteTelemetry {
                provider_path: "rules-fallback".to_string(),
                confidence_percent: 100,
                latency_ms: start.elapsed().as_millis() as u64,
                used_fallback: true,
                attempts: 0,
                timeout_ms_bound: bounded_timeout_ms,
                retries_bound: bounded_retries,
                last_error: None,
            },
        }
    }
}

pub fn default_route_decision() -> RouteDecision {
    RouteDecision {
        route: RouteType::RuleOnly,
        requires_memory_search: false,
        requires_approval: false,
        rationale: "default rules-first route".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{NanoBudget, NanoClassifier, RequestIntent, RouteType, Router};
    use crate::providers::{ApiNanoProvider, NanoProvider};

    struct FakeNano;

    impl NanoClassifier for FakeNano {
        fn classify(&self, request: &str) -> RequestIntent {
            if request.contains("tool") {
                return RequestIntent::RunTool;
            }
            RequestIntent::Unknown
        }
    }

    #[test]
    fn test_rules_route_retrieval() {
        let router = Router::new(false);
        let decision = router.route_rules_only("search docs for cargo");
        assert_eq!(decision.route, RouteType::Retrieval);
        assert!(decision.requires_memory_search);
    }

    #[test]
    fn test_nano_path_requires_approval_for_tool() {
        let router = Router::new(true);
        let decision = router.route_with_optional_nano("run tool now", Some(&FakeNano));
        assert_eq!(decision.route, RouteType::RuleWithNano);
        assert!(decision.requires_approval);
    }

    #[test]
    fn test_provider_path_uses_provider_intent() {
        let router = Router::new(true);
        let provider = ApiNanoProvider::new("demo-model");
        let decision = router.route_with_provider_budget(
            "please search docs",
            Some(&provider),
            NanoBudget::default(),
        );
        assert_eq!(decision.decision.route, RouteType::RuleWithNano);
        assert!(decision.decision.requires_memory_search);
        assert!(provider.name().contains("api"));
        assert_eq!(decision.telemetry.provider_path, "api-nano");
    }

    #[test]
    fn test_provider_budget_fallback_is_deterministic() {
        let router = Router::new(true);
        let provider = ApiNanoProvider::new("demo-model");
        let routed = router.route_with_provider_budget(
            "execute nano-fail force fallback",
            Some(&provider),
            NanoBudget {
                timeout_ms: 100,
                retries: 1,
            },
        );
        assert_eq!(routed.decision.route, RouteType::ToolExecution);
        assert!(routed.telemetry.used_fallback);
        assert_eq!(routed.telemetry.attempts, 2);
    }

    #[test]
    fn test_provider_budget_timeout_zero_forces_immediate_fallback() {
        let router = Router::new(true);
        let provider = ApiNanoProvider::new("demo-model");
        let routed = router.route_with_provider_budget(
            "execute nano-fail immediate timeout",
            Some(&provider),
            NanoBudget {
                timeout_ms: 0,
                retries: 5,
            },
        );
        assert!(routed.telemetry.used_fallback);
        assert_eq!(routed.telemetry.attempts, 1);
    }

    #[test]
    fn test_provider_budget_is_hard_bounded() {
        let router = Router::new(true);
        let provider = ApiNanoProvider::new("demo-model");
        let routed = router.route_with_provider_budget(
            "execute nano-fail hard bound",
            Some(&provider),
            NanoBudget {
                timeout_ms: 100_000,
                retries: 50,
            },
        );
        assert_eq!(routed.telemetry.timeout_ms_bound, 2_000);
        assert_eq!(routed.telemetry.retries_bound, 3);
        assert!(routed.telemetry.last_error.is_some());
    }

    #[test]
    fn test_route_with_provider_budget_soak_is_stable() {
        let router = Router::new(true);
        let provider = ApiNanoProvider::new("demo-model");
        for _ in 0..500 {
            let routed = router.route_with_provider_budget(
                "search docs for architecture",
                Some(&provider),
                NanoBudget {
                    timeout_ms: 300,
                    retries: 1,
                },
            );
            assert_eq!(routed.decision.route, RouteType::RuleWithNano);
            assert!(!routed.telemetry.used_fallback);
        }
    }
}
