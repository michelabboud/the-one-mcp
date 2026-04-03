use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;

use crate::error::CoreError;

static TELEMETRY_INIT: OnceLock<()> = OnceLock::new();

pub fn init_telemetry(default_level: &str, json_logs: bool) -> Result<bool, CoreError> {
    if TELEMETRY_INIT.get().is_some() {
        return Ok(false);
    }

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default_level))
        .map_err(|err| CoreError::InvalidProjectConfig(format!("invalid log filter: {err}")))?;

    let result = if json_logs {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .try_init()
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).try_init()
    };

    match result {
        Ok(()) => {
            let _ = TELEMETRY_INIT.set(());
            Ok(true)
        }
        Err(err) => {
            if TELEMETRY_INIT.get().is_some() {
                Ok(false)
            } else {
                Err(CoreError::InvalidProjectConfig(format!(
                    "telemetry initialization failed: {err}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::init_telemetry;

    #[test]
    fn test_init_telemetry_is_idempotent() {
        let _first = init_telemetry("info", false).expect("first init should succeed");
        let second = init_telemetry("debug", true).expect("second init should succeed");
        assert!(!second);
    }
}
