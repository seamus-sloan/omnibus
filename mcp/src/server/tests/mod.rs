//! Tests for the MCP tool layer, split by tool family into the sibling
//! modules below; the stub `/api/*` services and expected-tool lists they
//! share live here. Each family's router is exercised against an axum stub
//! standing in for the REST surface, so no live server is needed.

mod checkin;
mod read;
mod shelves;

use std::sync::Arc;

use super::*;
use crate::config::Config;

fn offline_server() -> OmnibusMcp {
    let client = OmnibusClient::new(Config {
        base_url: "http://127.0.0.1:1".into(),
        username: "reader".into(),
        password: "correct horse battery".into(),
    })
    .unwrap();
    OmnibusMcp::new(Arc::new(client))
}

/// `unwrap_err` needs `T: Debug`, which `rmcp::Json<T>` does not implement —
/// this is the same unwrap with the bound the tool results can satisfy.
trait ExpectErrData<E> {
    fn expect_err_data(self) -> E;
}

impl<T, E> ExpectErrData<E> for Result<T, E> {
    fn expect_err_data(self) -> E {
        match self {
            Err(e) => e,
            Ok(_) => panic!("expected an error"),
        }
    }
}
