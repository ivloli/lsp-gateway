use crate::models::ModelProvider;
use crate::{routing::frp_route::FrpNode, storage::CacheService};
use moka::future::Cache;
use rand::Rng;
use reqwest;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::Deserialize;
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};
use xxhash_rust::xxh64::xxh64;

#[derive(Deserialize)]
struct Proxy {
    name: String,
    status: String,
    #[serde(rename = "curConns")]
    cur_conns: u32,
}

#[derive(Deserialize)]
struct Response {
    proxies: Vec<Proxy>,
}

pub fn parse_proxies(
    json_str: &str,
) -> Result<Vec<(String, String, u32)>, Box<dyn std::error::Error + Send>> {
    let response: Response = serde_json::from_str(json_str)
        .map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;
    let result = response
        .proxies
        .into_iter()
        .map(|p| (p.name, p.status, p.cur_conns))
        .collect();
    Ok(result)
}

fn format_map_truncated(
    map: &HashMap<String, Vec<String>>,
    max_entries: usize,
    max_items_per_entry: usize,
) -> String {
    let total = map.len();
    if total == 0 {
        return "{}".to_string();
    }

    let mut parts = Vec::new();
    for (i, (key, values)) in map.iter().enumerate() {
        if i >= max_entries {
            parts.push(format!("... and {} more entries", total - max_entries));
            break;
        }

        let values_str = if values.len() <= max_items_per_entry {
            format!("[{}]", values.join(", "))
        } else {
            let shown: Vec<_> = values.iter().take(max_items_per_entry).cloned().collect();
            format!(
                "[{}, ... and {} more]",
                shown.join(", "),
                values.len() - max_items_per_entry
            )
        };

        parts.push(format!("{}: {}", key, values_str));
    }

    format!("{{ {} }} (total: {} entries)", parts.join(", "), total)
}

pub struct FrpManager {
    pub url: String,
    pub passwd: String,
    pub conns_limit: u32,
    frp_cache: Cache<String, FrpNode>,
    cluster_to_nodes: Arc<RwLock<HashMap<String, Vec<String>>>>,
    frpmodel_to_nodes: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Global lock to provide strong consistency between readers and the
    /// updater. Readers acquire a read guard for the duration of their
    /// operation; the updater acquires a write guard for the full update.
    /// This is a simple, conservative mechanism that allows us to avoid
    /// concurrent reads while an update is in progress (accepting some
    /// performance loss as requested).
    update_lock: Arc<RwLock<()>>,
}

impl FrpManager {
    pub async fn new(
        url: String,
        passwd: String,
        conns_limit: u32,
        cache_service: Arc<CacheService>,
    ) -> Self {
        let manager = Self {
            url,
            passwd,
            conns_limit,
            frp_cache: cache_service.frp_node_cache.clone(), // 直接使用缓存实例
            cluster_to_nodes: Arc::new(RwLock::new(HashMap::new())),
            frpmodel_to_nodes: Arc::new(RwLock::new(HashMap::new())),
            update_lock: Arc::new(RwLock::new(())),
        };

        // Perform initial FRP nodes update to ensure data is loaded before service starts
        manager.update_frp_nodes().await;

        manager
    }

    pub async fn select_provider_node(
        &self,
        provider: &ModelProvider,
        key: Option<&str>,
    ) -> Option<ModelProvider> {
        // Check if provider has frpmodel configured
        let frpmodel = provider.frpmodel.as_ref()?;
        // Acquire the global read lock for the entire selection operation so
        // no concurrent `update_frp_nodes` runs while we're selecting.
        let _read_guard = self.update_lock.read().await;

        // Get nodes by frpmodel
        let nodes = self.get_nodes_by_model(frpmodel).await;

        if nodes.is_empty() {
            warn!("No FRP nodes found for frpmodel: {}", frpmodel);
            return None;
        } else {
            debug!("FRP nodes found {} for frpmodel: {}", nodes.len(), frpmodel);
        }

        // Use weighted rendezvous hash to select a node
        let selected_node = Self::weighted_rendezvous_hash(key, &nodes)?;

        // First check if we need to perform a health check on the selected node
        // If health check is not needed, and the node is already active, use it directly
        if !selected_node.should_check_health() && selected_node.active {
            let mut new_provider = provider.clone();
            new_provider.base_url = selected_node.get_url();
            return Some(new_provider);
        }

        // When a health check is needed we may update the node's health and
        // write it back to the cache. We still hold the global read lock here
        // (so updates are mutually exclusive with `update_frp_nodes`).
        if let Some(mut node) = self.frp_cache.get(&selected_node.name).await {
            node.update_health_status().await;
            // write back updated node into cache while still under read guard
            self.frp_cache
                .insert(selected_node.name.clone(), node.clone())
                .await;

            if !node.active {
                warn!("FRP node {} is not active", selected_node.name);
                return None;
            }

            // Create a new ModelProvider with the selected node's URL
            let mut new_provider = provider.clone();
            new_provider.base_url = node.get_url();

            Some(new_provider)
        } else {
            warn!("FRP node {} not found in cache", selected_node.name);
            None
        }
    }

    pub async fn get_frp_node(&self, name: &str) -> Option<FrpNode> {
        self.frp_cache.get(name).await
    }

    /// Get FRP nodes by cluster name
    pub async fn get_nodes_by_cluster(&self, cluster: &str) -> Vec<FrpNode> {
        // Acquire global read lock to prevent concurrent updates while reading.
        let _read_guard = self.update_lock.read().await;

        // Clone the node name list under the cluster map lock, then perform
        // cache lookups while still holding the global read lock to guarantee
        // consistency between mapping and cache contents.
        let node_names_opt = {
            let cluster_map = self.cluster_to_nodes.read().await;
            cluster_map.get(cluster).cloned()
        };

        if let Some(node_names) = node_names_opt {
            let mut nodes = Vec::new();
            for name in node_names {
                if let Some(node) = self.frp_cache.get(&name).await {
                    if node.active {
                        nodes.push(node);
                    }
                }
            }
            nodes
        } else {
            Vec::new()
        }
    }

    /// Get FRP nodes by frpmodel name
    pub async fn get_nodes_by_model(&self, frpmodel: &str) -> Vec<FrpNode> {
        // Acquire global read lock to ensure reads and updates are mutually
        // exclusive (strong consistency). This is intentionally conservative.
        let _read_guard = self.update_lock.read().await;

        let node_names_opt = {
            let model_map = self.frpmodel_to_nodes.read().await;
            model_map.get(frpmodel).cloned()
        };

        if let Some(node_names) = node_names_opt {
            let mut nodes = Vec::new();
            for name in node_names {
                if let Some(node) = self.frp_cache.get(&name).await {
                    if node.active && node.cur_conns < self.conns_limit {
                        nodes.push(node);
                    }
                }
            }
            nodes
        } else {
            Vec::new()
        }
    }

    async fn update_frp_nodes(&self) {
        // Acquire write lock for the whole update to prevent concurrent reads
        // from observing a partially-updated state. This is the simple,
        // conservative approach requested by the user (accepts performance hit).
        let _write_guard = self.update_lock.write().await;

        let client = reqwest::Client::new();
        let full_url = format!("{}/api/proxy/http", self.url);
        let mut headers = HeaderMap::new();
        let auth_value = format!("Basic {}", self.passwd);
        if let Ok(auth_header) = HeaderValue::from_str(&auth_value) {
            headers.insert(AUTHORIZATION, auth_header);
        }

        match client.get(&full_url).headers(headers).send().await {
            Ok(response) => match response.text().await {
                Ok(text) => {
                    match parse_proxies(&text) {
                        Ok(proxies) => {
                            debug!("Successfully parsed {} proxies", proxies.len());

                            // Acquire write locks on the maps before performing update
                            // operations. We already hold the global write guard so
                            // no readers are active.
                            let mut cluster_map = self.cluster_to_nodes.write().await;
                            let mut frpmodel_map = self.frpmodel_to_nodes.write().await;

                            // Clear existing data
                            self.frp_cache.invalidate_all();
                            self.frp_cache.run_pending_tasks().await;
                            cluster_map.clear();
                            frpmodel_map.clear();

                            for (name, status, cur_conns) in proxies {
                                // Split only the first 3 segments, merge the rest into the last part
                                let splits: Vec<&str> = name.splitn(3, '-').collect();
                                if splits.len() < 3 {
                                    warn!("Invalid FRP node name: {}", name);
                                    continue;
                                }
                                let frpmodel = splits[0];
                                let cluster = splits[1];
                                let hostname = splits[2];
                                if status == "online" {
                                    let node =
                                        FrpNode::new(frpmodel, cluster, hostname, &name, cur_conns);
                                    // insert into cache while still holding the global
                                    // write guard so readers are excluded.
                                    self.frp_cache.insert(name.clone(), node).await;
                                    debug!(
                                        "Cached FRP node: {} with {} connections",
                                        name, cur_conns
                                    );

                                    // Update cluster_to_nodes mapping with node name reference
                                    cluster_map
                                        .entry(cluster.to_string())
                                        .or_insert_with(Vec::new)
                                        .push(name.clone());

                                    // Update frpmodel_to_nodes mapping with node name reference
                                    frpmodel_map
                                        .entry(frpmodel.to_string())
                                        .or_insert_with(Vec::new)
                                        .push(name);
                                } else {
                                    debug!("FRP node {} is offline, skipping cache update", name);
                                }
                            }
                            info!(
                                "FRP nodes update completed. Total cached nodes: {}",
                                self.frp_cache.entry_count()
                            );
                            info!(
                                "Cluster to nodes mapping: {}",
                                format_map_truncated(&cluster_map, 5, 3)
                            );
                            info!(
                                "Frpmodel to nodes mapping: {}",
                                format_map_truncated(&frpmodel_map, 5, 3)
                            );
                            // Write locks are automatically released here
                        }
                        Err(e) => {
                            error!("Failed to parse proxies {}: {}", text, e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to get response text: {}", e);
                }
            },
            Err(e) => {
                error!("Failed to fetch FRP nodes from {}: {}", full_url, e);
            }
        }
    }

    pub fn start_update_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(15));

            loop {
                interval.tick().await;
                self.update_frp_nodes().await;
            }
        })
    }

    /// Get all FRP nodes as a JSON string
    pub async fn get_all_nodes_as_json(&self) -> Result<String, anyhow::Error> {
        // Acquire global read lock to ensure consistency
        let _read_guard = self.update_lock.read().await;

        // Collect all nodes from the cache
        let nodes: Vec<_> = self
            .frp_cache
            .iter()
            .map(|(_key, value)| value.clone())
            .collect();

        // Serialize to JSON
        let json = serde_json::to_string(&nodes)?;
        Ok(json)
    }

    pub fn weighted_rendezvous_hash<'a>(
        key: Option<&str>,
        nodes: &'a [FrpNode],
    ) -> Option<&'a FrpNode> {
        // Return None directly for empty nodes
        if nodes.is_empty() {
            return None;
        }

        // If key is None, use weighted random selection
        if let Some(key) = key {
            info!(
                "Selecting FRP node using rendezvous hashing with key: {}",
                key
            );
            // Rendezvous hashing implementation
            let mut best_node: Option<&FrpNode> = None;
            let mut best_score: f64 = f64::NEG_INFINITY;

            // Reuse a byte buffer to avoid allocating String each time (micro-optimization)
            let mut buf = Vec::with_capacity(key.len() + 1 + 32);

            for node in nodes {
                if node.weight == 0 || node.active == false {
                    continue;
                }

                buf.clear();
                buf.extend_from_slice(key.as_bytes());
                buf.push(b'-');
                buf.extend_from_slice(node.name.as_bytes());

                let raw_hash: u64 = xxh64(&buf, 0);
                // Safer and more explicit floating point conversion
                let hash_f = (raw_hash as f64) / (u64::MAX as f64);

                // Score formula: hash^(1/weight)
                let score = hash_f.powf(1.0 / (node.weight as f64));
                info!("Rendezvous hash score for node {}: {}", node.name, score);

                if best_node.is_none() || score > best_score {
                    best_score = score;
                    best_node = Some(node);
                }
            }

            best_node
        } else {
            info!("No key provided, selecting randomly");
            // Weighted random selection when no key is provided
            let total_weight: usize = nodes
                .iter()
                .filter(|node| node.weight > 0 && node.active)
                .map(|node| node.weight as usize)
                .sum();

            if total_weight == 0 {
                return None;
            }

            // Generate a random number in [0, total_weight)
            let mut rng = rand::thread_rng();
            let random_weight = rng.gen_range(0..total_weight);

            // Select node based on weighted random selection
            let mut cumulative_weight: usize = 0;
            for node in nodes {
                if node.weight == 0 || !node.active {
                    continue;
                }

                cumulative_weight += node.weight as usize;
                if random_weight < cumulative_weight {
                    return Some(node);
                }
            }

            // Fallback - should not happen but return first active node if it does
            let fallback_result = nodes.iter().find(|node| node.weight > 0 && node.active);

            if fallback_result.is_some() {
                warn!("Weighted random selection fell back to first active node due to algorithm completion");
            } else {
                warn!("Weighted random selection failed completely - no active nodes with positive weight found");
            }

            fallback_result
        }
    }
}
