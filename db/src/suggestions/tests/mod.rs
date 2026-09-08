//! Tests for the suggestions module, split by layer into the sibling
//! modules below: the pure cache state machine and filters, the cache
//! CRUD, the Hardcover GraphQL client against `wiremock`, and the
//! end-to-end cascade.

mod cache;
mod cascade;
mod hardcover;
mod state;

use wiremock::MockServer;

use crate::suggestions::hardcover::HardcoverConfig;

fn config_for(server: &MockServer) -> HardcoverConfig {
    HardcoverConfig {
        base_url: server.uri(),
        api_key: "test-key".to_string(),
        timeout: std::time::Duration::from_secs(5),
    }
}
