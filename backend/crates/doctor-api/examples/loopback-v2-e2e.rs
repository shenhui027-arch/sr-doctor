//! Runner-owned development endpoint, separate from the user's port-8000 app.
//! Release acceptance must still launch the packaged production executable.
use doctor_api::{ApiConfig, ApiState, serve};
use std::{net::SocketAddr, path::PathBuf};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::args()
        .nth(1)
        .ok_or("expected explicit loopback port")?
        .parse::<u16>()?;
    if port == 0 {
        return Err("port must be nonzero".into());
    }
    let frontend_dist = std::env::var_os("DOCTOR_FRONTEND_DIST").map(PathBuf::from);
    let state = ApiState::new(ApiConfig {
        bind_addr: SocketAddr::from(([127, 0, 0, 1], port)),
        frontend_dist,
        ..ApiConfig::default()
    })?;
    serve(state).await?;
    Ok(())
}
