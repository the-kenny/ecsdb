use std::net::SocketAddr;

use axum::routing::get;
use tracing::info;

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    use axum::Router;

    let service =
        ecsdb_web::service(|_req: &http::Request<_>| ecsdb::Ecs::open("scratch/test.sqlite"));
    let service = tower::ServiceBuilder::new().service(service);

    let app = Router::new()
        .route("/test", get("test route"))
        .fallback_service(service);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}
