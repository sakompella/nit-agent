use std::time::Duration;

use axum::Router;
use tokio::net::TcpListener;

/// Bind to an ephemeral port, serve `router` for up to 10 s, and return the
/// base URL (e.g. `"http://127.0.0.1:12345"`).
pub async fn spawn_router(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener
        .local_addr()
        .expect("test listener should have a local address");
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                tokio::time::sleep(Duration::from_secs(10)).await;
            })
            .await
            .expect("test server should run");
    });
    format!("http://{}:{}", address.ip(), address.port())
}

/// Extract the payload from `data: <payload>` lines in an SSE response body.
pub fn sse_data_events(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(ToOwned::to_owned)
        .collect()
}
