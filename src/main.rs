use anyhow::Context;
use anyhow::Result;
use axum::{
    Router,
    routing::{get, post},
};
use serde::Deserialize;

const WEBSIGHT_ADDR: &str = "0.0.0.0:3000";
const OWNTRACKS_ADDR: &str = "10.68.39.1:1234";

#[derive(Debug, Deserialize)]
struct LocationPayload {
    /// Unix epoch timestamp.
    tst: i64,
    lat: f64,
    lon: f64,
    /// Accuracy in metres. Unset if 0.
    acc: Option<u64>,
    /// Altitude in metres.
    alt: Option<i64>,
    /// Altitude accuracy
    vac: Option<i64>,
    /// Velocity (km/h)
    vel: Option<f64>,
    /// Battery percent.
    bat: Option<i64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Welcome to Tage's tur!");

    tokio::try_join!(websight(), owntracks())?;
    Ok(())
}

async fn websight() -> Result<()> {
    // build our application with a single route
    let app = Router::new().route("/", get(|| async { "Hello, World!" }));

    let listener = tokio::net::TcpListener::bind(WEBSIGHT_ADDR)
        .await
        .context("can't bind to websight address")?;
    axum::serve(listener, app)
        .await
        .context("websight crashed")?;
    Ok(())
}

async fn owntracks() -> Result<()> {
    let app = Router::new().route("/", post(handle_owntracks));

    let listener = tokio::net::TcpListener::bind(OWNTRACKS_ADDR)
        .await
        .context("can't bind to owntracks address")?;
    axum::serve(listener, app)
        .await
        .context("owntracks server crashed")?;
    Ok(())
}

async fn handle_owntracks(axum::Json(payload): axum::Json<serde_json::Value>) {
    log::debug!("Got: {:#?}", payload);
    println!("Got: {}", payload["_type"]);
    if payload["_type"] == "location" {
        let Ok(loc) = <LocationPayload as Deserialize>::deserialize(&payload)
            .inspect_err(|e| log::error!("Failed to parse location payload: {e:#?}\n{payload:#?}"))
        else {
            return;
        };
        println!("Got location: {:#?}", loc);
    }
}
