//! Is a store reachable? A TCP connect to its API host with a short timeout.
//! Without a network the name does not resolve or nothing answers, so an
//! offline launch never waits for a helper's own (much longer) timeouts.

use std::time::Duration;

use crate::types::Runner;

const TIMEOUT: Duration = Duration::from_secs(4);

fn host(runner: Runner) -> Option<&'static str> {
    match runner {
        Runner::Legendary => Some("account-public-service-prod03.ol.epicgames.com:443"),
        Runner::Gog => Some("auth.gog.com:443"),
        Runner::Sideload => None,
    }
}

/// True when the store's servers answer. Sideloaded games have none.
pub async fn reachable(runner: Runner) -> bool {
    let Some(host) = host(runner) else {
        return true;
    };
    let ok = matches!(
        tokio::time::timeout(TIMEOUT, tokio::net::TcpStream::connect(host)).await,
        Ok(Ok(_))
    );
    if !ok {
        tracing::info!(store = runner.id(), "store not reachable; working offline");
    }
    ok
}
