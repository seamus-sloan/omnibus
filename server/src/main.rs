use std::net::SocketAddr;

use omnibus::{backend, db};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://omnibus.db?mode=rwc".to_string());
    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("omnibus=debug".parse()?)
                .add_directive("tower_http=debug".parse()?),
        )
        .init();

    let pool = db::init_db(&database_url).await?;
    let state = backend::AppState::new(pool);
    let app = backend::router(state);

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("Server running at http://{}", listener.local_addr()?);

    axum::serve(listener, app).await?;
    Ok(())
}
