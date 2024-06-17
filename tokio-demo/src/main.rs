use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use log::{debug, info};
use picoserve::{
    extract::State,
    response::{DebugValue, IntoResponse},
    routing::{get, parse_path_segment},
};

struct Control {
    led2: bool,
}

type SharedControl = Rc<RefCell<Control>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    info!("App started");

    let port = 8000;

    let app = std::rc::Rc::new(
        picoserve::Router::new()
            .route(
                "/",
                get(|| picoserve::response::File::html(include_str!("index.html"))),
            )
            .route(
                "/index.css",
                get(|| picoserve::response::File::css(include_str!("index.css"))),
            )
            .route(
                "/index.js",
                get(|| picoserve::response::File::javascript(include_str!("index.js"))),
            )
            .route(
                ("/toggle_led", parse_path_segment()),
                get(
                    |led_type: u8, State(state): State<SharedControl>| async move {
                        info!("Toggling LED{}", led_type);
                        let led2 = &mut state.borrow_mut().led2;
                        *led2 = !*led2;
                        // debug!("State LED value after toggle: {}", state.borrow().led2);
                        debug!("LED value after toggle: {}", led2);
                        if *led2 {
                            "ON"
                        } else {
                            "OFF"
                        }
                    },
                ),
            ),
    );

    let config = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Some(Duration::from_secs(5)),
        read_request: Some(Duration::from_secs(1)),
        write: Some(Duration::from_secs(1)),
    })
    .keep_connection_alive();

    let shared_control = Rc::new(RefCell::new(Control { led2: true }));

    let socket = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 8000)).await?;

    info!("http://localhost:{port}/");

    tokio::task::LocalSet::new()
        .run_until(async {
            loop {
                let (stream, remote_address) = socket.accept().await?;

                info!("Connection from {remote_address}");

                let app = app.clone();
                let config = config.clone();
                let shared_control_clone = shared_control.clone();

                tokio::task::spawn_local(async move {
                    picoserve::serve_with_state(
                        &app,
                        &config,
                        &mut [0; 2048],
                        stream,
                        &shared_control_clone
                    )
                    .await
                });
            }
        })
        .await
}
