use std::net::SocketAddr;

use hyper::server::conn::http1;
use tokio::net::TcpListener;
use tracing::{error, info};

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let addr: SocketAddr = ([127, 0, 0, 1], 3000).into();

    // Bind to the port and listen for incoming TCP connections
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on http://{}", addr);
    loop {
        let service = ecsdb_web::service("/", |_req: &http::Request<_>| {
            ecsdb::Ecs::open("scratch/test.sqlite")
        });
        let service = tower::ServiceBuilder::new().service(service);

        let service = hyper_util::service::TowerToHyperService::new(service);

        let (tcp, _) = listener.accept().await?;
        let io = hyper_util::rt::TokioIo::new(tcp);

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .timer(hyper_util::rt::TokioTimer::new())
                .serve_connection(io, service)
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}
