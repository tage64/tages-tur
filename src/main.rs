use anyhow::{Context, Result, bail};
use askama::Template;
use axum::{
    Router,
    response::Html,
    routing::{get, post},
};
use jord::spherical::Sphere;
use jord::{Angle, LatLong};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use time::Time;

const WEBSIGHT_ADDR: &str = "0.0.0.0:3000";
const OWNTRACKS_ADDR: &str = "10.68.39.1:1234";

const STATE_FILE: &str = "state.json";

static GOAL: LazyLock<LatLong> = LazyLock::new(|| {
    LatLong::new(
        Angle::from_degrees(59.314754),
        Angle::from_degrees(18.067139),
    )
});
static RESET_TIME: LazyLock<Time> = LazyLock::new(|| Time::from_hms(7, 30, 0).expect(""));
static TIMEZONE: LazyLock<time::UtcOffset> =
    LazyLock::new(|| time::UtcOffset::from_hms(2, 0, 0).unwrap());

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(unused)]
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

#[derive(Template)]
enum Info {
    #[template(path = "running.html")]
    Running {
        lat: f64,
        lon: f64,
        vel: f64,
        distance: f64,
        goal_lat: f64,
        goal_lon: f64,
        distance_to_goal: f64,
        start_time: String,
        curr_time: String,
    },
    #[template(path = "reached_goal.html")]
    ReachedGoal {
        distance: f64,
        lat: f64,
        lon: f64,
        start_time: String,
        goal_time: String,
    },
    #[template(path = "error.html")]
    Error { error: anyhow::Error },
}

#[derive(Debug, Serialize, Deserialize)]
struct State {
    finished: bool,
    reached_goal: bool,
    start_time: time::OffsetDateTime,
    last_location_time: time::OffsetDateTime,
    distance: f64,
    last_pos: LatLong,
    last_acc: f64,
    loc: LocationPayload,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Welcome to Tage's tur!");
    println!("The clock is {} UTC", time::UtcDateTime::now().time());

    let state = Arc::new(Mutex::<Option<State>>::new(None));

    tokio::try_join!(websight(state.clone()), owntracks(state.clone()), async {
        save_state(state).await;
        Ok(())
    },)?;
    Ok(())
}

async fn websight(state: Arc<Mutex<Option<State>>>) -> Result<()> {
    let state_copy = state.clone();
    let state_copy_2 = state.clone();

    // build our application with a single route
    let app = Router::new()
        .route(
            "/",
            get(async move || {
                Html(render_websight(&state).await.unwrap_or_else(|error| {
                    Info::Error { error }
                        .render()
                        .unwrap_or_else(|e| format!("Template error: {e}"))
                }))
            }),
        )
        .route(
            "/goal",
            get(async move || {
                if let Some(s) = &mut *state_copy.lock().unwrap() {
                    s.finished = true;
                    s.reached_goal = true;
                }
                "You have now told the system that Tage reached the goal!"
            }),
        )
        .route(
            "/ungoal",
            get(async move || {
                if let Some(s) = &mut *state_copy_2.lock().unwrap() {
                    s.finished = false;
                    s.reached_goal = false;
                }
                "We unreached the goal!"
            }),
        );

    let listener = tokio::net::TcpListener::bind(WEBSIGHT_ADDR)
        .await
        .context("can't bind to websight address")?;
    axum::serve(listener, app)
        .await
        .context("websight crashed")?;
    Ok(())
}

async fn render_websight(state: &Arc<Mutex<Option<State>>>) -> anyhow::Result<String> {
    let state_lock = state.lock().unwrap();
    let Some(state) = state_lock.as_ref() else {
        bail!("No position yet")
    };

    let info = if state.finished {
        Info::ReachedGoal {
            distance: (state.distance / 100.0).round() / 10.0,
            lat: state.last_pos.latitude().as_degrees(),
            lon: state.last_pos.longitude().as_degrees(),
            start_time: state
                .start_time
                .format(
                    &time::format_description::parse_borrowed::<1>("[hour]:[minute]:[second]")
                        .unwrap(),
                )
                .unwrap(),
            goal_time: state
                .last_location_time
                .format(
                    &time::format_description::parse_borrowed::<1>("[hour]:[minute]:[second]")
                        .unwrap(),
                )
                .unwrap(),
        }
    } else {
        let pos = LatLong::from_degrees(state.loc.lat, state.loc.lon);
        let now = time::OffsetDateTime::from_unix_timestamp(state.loc.tst)
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
            .to_offset(*TIMEZONE);
        Info::Running {
            lat: state.loc.lat,
            lon: state.loc.lon,
            vel: state.loc.vel.unwrap_or(0.0),
            distance: (state.distance / 100.0).round() / 10.0,
            goal_lat: GOAL.latitude().as_degrees(),
            goal_lon: GOAL.longitude().as_degrees(),
            distance_to_goal: (Sphere::EARTH
                .distance(pos.to_nvector(), GOAL.to_nvector())
                .as_metres()
                / 100.0)
                .round()
                / 10.0,
            start_time: state
                .start_time
                .format(
                    &time::format_description::parse_borrowed::<1>("[hour]:[minute]:[second]")
                        .unwrap(),
                )
                .unwrap(),
            curr_time: now
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        }
    };

    info.render().context("failed to render")
}

async fn owntracks(state: Arc<Mutex<Option<State>>>) -> Result<()> {
    let app = Router::new().route("/", post(async move |x| handle_owntracks(&state, x).await));

    let listener = tokio::net::TcpListener::bind(OWNTRACKS_ADDR)
        .await
        .context("can't bind to owntracks address")?;
    axum::serve(listener, app)
        .await
        .context("owntracks server crashed")?;
    Ok(())
}

async fn handle_owntracks(
    state: &Arc<Mutex<Option<State>>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) {
    println!("Got: {:#?}", payload);
    let loc = if payload["_type"] == "location" {
        let Ok(loc) = <LocationPayload as Deserialize>::deserialize(&payload)
            .inspect_err(|e| log::error!("Failed to parse location payload: {e:#?}\n{payload:#?}"))
        else {
            return;
        };
        loc
    } else {
        return;
    };

    let pos = LatLong::from_degrees(loc.lat, loc.lon);
    let acc = loc.acc.unwrap_or(0) as f64;
    let now = time::OffsetDateTime::from_unix_timestamp(loc.tst)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
        .to_offset(*TIMEZONE);

    let mut state_lock = state.lock().unwrap();
    if let Some(state) = &mut *state_lock {
        let reset_time = state.last_location_time.replace_time(*RESET_TIME);
        if state.last_location_time < reset_time && reset_time <= now {
            // Drop the old state.
            *state_lock = None;
            println!("Dropping state");
        } else {
            let distance_to_last_pos = Sphere::EARTH
                .distance(state.last_pos.to_nvector(), pos.to_nvector())
                .as_metres();
            if distance_to_last_pos >= state.last_acc.max(acc) {
                state.distance += distance_to_last_pos;
                state.last_pos = pos;
                state.last_acc = acc;
                state.last_location_time = now;
            }

            state.loc = loc.clone();
        }
    }

    if state_lock.is_none() {
        *state_lock = Some(State {
            finished: false,
            reached_goal: false,
            start_time: now,
            last_location_time: now,
            distance: 0.0,
            last_pos: pos,
            last_acc: acc,
            loc,
        });
    }
}

async fn save_state(state: Arc<Mutex<Option<State>>>) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let json = if let Some(state) = state.lock().unwrap().as_ref() {
            let Ok(json) = serde_json::to_string(state) else {
                continue;
            };
            json
        } else {
            continue;
        };
        std::fs::write(STATE_FILE, json + "\n").ok();
    }
}
