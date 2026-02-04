use std::{net::SocketAddr, path::PathBuf};

use axum::routing::get;
use tracing::{info, warn};

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let db_file = match &std::env::args().collect::<Box<[_]>>()[..] {
        [] | [_] => PathBuf::from("scratch/test.sqlite"),
        [_, path] => PathBuf::from(path),
        args => {
            warn!(?args, "Invalid command line");
            eprintln!("Usage: {} DBPATH", env!("CARGO_CRATE_NAME"));
            return Ok(());
        }
    };

    use axum::Router;

    let service = ecsdb_web::service("/ecsdb/", move |_req: &http::Request<_>| {
        ecsdb::Ecs::open(&db_file)
    });
    let service = tower::ServiceBuilder::new().service(service);

    let app = Router::new()
        .route("/test", get("test route"))
        .nest_service("/ecsdb", service);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}
