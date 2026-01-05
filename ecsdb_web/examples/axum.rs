use std::convert::Infallible;
use std::net::SocketAddr;

use tracing::info;

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    use axum::Router;

    let service = tower::ServiceBuilder::new()
        .map_err(|e| -> Infallible { panic!("{e}") })
        .service(ecsdb_web::Service);

    // let service = hyper_util::service::TowerToHyperService::new(service);
    let app = Router::new().nest_service("/hello", service);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}
