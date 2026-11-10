use once_cell::sync::Lazy;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Domain suffix appended to FRP node names, configurable via `FRP_SERVER_SUFFIX`.
static SERVER_SUFFIX: Lazy<String> = Lazy::new(|| {
    std::env::var("FRP_SERVER_SUFFIX").unwrap_or_else(|_| ".frp.example.com".to_string())
});

/// Scheme used when building FRP node URLs, configurable via `FRP_SCHEME`.
static FRP_SCHEME: Lazy<String> =
    Lazy::new(|| std::env::var("FRP_SCHEME").unwrap_or_else(|_| "http".to_string()));

pub fn frp_server_suffix() -> &'static str {
    SERVER_SUFFIX.as_str()
}

#[derive(Debug, Clone, Serialize)]
pub struct FrpNode {
    pub frpmodel: String,
    pub cluster: String,
    pub hostname: String,
    pub name: String,
    pub url: String,
    pub weight: u16,
    pub active: bool,
    pub cur_conns: u32,
    #[serde(skip)]
    last_health_check: Arc<AtomicU64>,
}

impl FrpNode {
    const HEALTH_CHECK_INTERVAL: u64 = 5; // 5 seconds

    pub fn new(frpmodel: &str, cluster: &str, hostname: &str, name: &str, cur_conns: u32) -> Self {
        Self {
            frpmodel: frpmodel.to_string(),
            cluster: cluster.to_string(),
            hostname: hostname.to_string(),
            name: name.to_string(),
            url: format!("{}{}", name, frp_server_suffix()),
            weight: 10,
            active: true,
            cur_conns,
            last_health_check: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn get_url(&self) -> String {
        format!(
            "{}://{}{}/api/v1",
            FRP_SCHEME.as_str(),
            self.name,
            frp_server_suffix()
        )
    }

    pub fn get_health_url(&self) -> String {
        format!(
            "{}://{}{}/health",
            FRP_SCHEME.as_str(),
            self.name,
            frp_server_suffix()
        )
    }

    /// Check if it's time to perform a health check
    /// Returns true if the HEALTH_CHECK_INTERVAL has passed since the last check
    pub fn should_check_health(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let last_check = self.last_health_check.load(Ordering::Relaxed);

        // If less than 5 seconds have passed since last check, skip the check
        now - last_check >= Self::HEALTH_CHECK_INTERVAL
    }

    pub async fn update_health_status(&mut self) {
        // If it's not time to check health yet, return early
        if !self.should_check_health() {
            return;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Update the last check time
        self.last_health_check.store(now, Ordering::Relaxed);

        let health_url = self.get_health_url();

        match reqwest::Client::new().get(&health_url).send().await {
            Ok(response) => {
                self.active = response.status().is_success();
            }
            Err(_) => {
                self.active = false;
            }
        }
    }
}
