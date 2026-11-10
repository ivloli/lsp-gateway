use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Generate a unique gateway ID based on hostname, PID, and timestamp
pub fn generate_gateway_id() -> String {
    let hostname = hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let pid = std::process::id();
    let timestamp = crate::utils::TimeUtils::current_timestamp_nanos();
    let random: u64 = rand::random();

    let mut hasher = DefaultHasher::new();
    hostname.hash(&mut hasher);
    pid.hash(&mut hasher);
    timestamp.hash(&mut hasher);
    random.hash(&mut hasher);

    let hash = hasher.finish();
    format!("gw-{:x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_gateway_id() {
        let id1 = generate_gateway_id();
        let id2 = generate_gateway_id();

        // IDs should be different (due to timestamp and random components)
        assert_ne!(id1, id2);

        // IDs should start with "gw-"
        assert!(id1.starts_with("gw-"));
        assert!(id2.starts_with("gw-"));

        // IDs should be reasonable length (not too short, not too long)
        assert!(id1.len() > 5);
        assert!(id1.len() < 50);

        println!("Generated IDs: {} and {}", id1, id2);
    }
}
