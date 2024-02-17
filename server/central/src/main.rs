#![feature(duration_constructors)]
#![allow(
    clippy::must_use_candidate,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::wildcard_imports
)]

use std::{error::Error, path::PathBuf, time::Duration};

use async_watcher::{notify::RecursiveMode, AsyncDebouncer};
use config::ServerConfig;
use db::GlobedDb;
use globed_shared::{
    get_log_level,
    logger::{error, info, log, warn, Logger},
    LogLevelFilter,
};
use rocket::fairing::AdHoc;
use state::{ServerState, ServerStateData};

pub mod config;
pub mod db;
pub mod ip_blocker;
pub mod state;
pub mod verifier;
pub mod web;

fn abort_misconfig() -> ! {
    error!("aborting launch due to misconfiguration.");
    std::process::exit(1);
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn Error>> {
    log::set_logger(Logger::instance("globed_central_server", false)).unwrap();

    if let Some(log_level) = get_log_level("GLOBED_LOG_LEVEL") {
        log::set_max_level(log_level);
    } else {
        log::set_max_level(LogLevelFilter::Warn); // we have to print these logs somehow lol
        error!("invalid value for the log level environment varaible");
        warn!("hint: possible values are 'trace', 'debug', 'info', 'warn', 'error', and 'none'.");
        abort_misconfig();
    }

    // config file

    let mut config_path =
        std::env::var("GLOBED_CONFIG_PATH").map_or_else(|_| std::env::current_dir().unwrap(), PathBuf::from);

    if config_path.is_dir() {
        config_path = config_path.join("central-conf.json");
    }

    let config = if config_path.exists() && config_path.is_file() {
        match ServerConfig::load(&config_path) {
            Ok(x) => x,
            Err(err) => {
                error!("failed to open/parse configuration file: {err}");
                warn!("hint: if you don't have anything important there, delete the file for a new template to be created.");
                warn!("hint: the faulty configuration resides at: {config_path:?}");
                abort_misconfig();
            }
        }
    } else {
        info!("Configuration file does not exist by given path, creating a template one.");

        let conf = ServerConfig::default();
        conf.save(&config_path)?;

        conf
    };

    // stupid rust

    let mnt_point = config.web_mountpoint.clone();

    let state_skey = config.secret_key.clone();
    let state_skey2 = config.secret_key2.clone();

    let state = ServerState::new(ServerStateData::new(config_path.clone(), config, &state_skey, &state_skey2));

    // config file watcher

    let (mut debouncer, mut file_events) =
        AsyncDebouncer::new_with_channel(Duration::from_secs(1), Some(Duration::from_secs(1))).await?;

    debouncer.watcher().watch(&config_path, RecursiveMode::NonRecursive)?;

    let watcher_state = state.clone();
    tokio::spawn(async move {
        while let Some(_event) = file_events.recv().await {
            let mut state = watcher_state.state_write().await;
            let cpath = state.config_path.clone();
            match state.config.reload_in_place(&cpath) {
                Ok(()) => {
                    info!("Successfully reloaded the configuration");
                    // set the maintenance flag appropriately
                    watcher_state.set_maintenance(state.config.maintenance);
                    watcher_state.inner.verifier.set_enabled(state.config.use_gd_api);
                }
                Err(err) => {
                    warn!("Failed to reload configuration: {}", err.to_string());
                }
            }
        }
    });

    // account verification stuff
    let av_state = state.inner.clone();
    let av_state2 = state.inner.clone();
    tokio::spawn(async move {
        av_state.verifier.run_refresher().await;
    });

    tokio::spawn(async move {
        av_state2.verifier.run_deleter().await;
    });

    // start up rocket

    let rocket = rocket::build()
        .mount(mnt_point, web::routes::build_router())
        .manage(state)
        .attach(GlobedDb::fairing())
        .attach(AdHoc::on_liftoff("Database migrations", |rocket| {
            Box::pin(async move {
                log::info!("Running migrations");
                let db = GlobedDb::get_one(rocket).await.unwrap();
                match db::run_migrations(&db).await {
                    Ok(()) => {}
                    Err(err) => {
                        error!("failed to run migrations: {err}");
                    }
                }
            })
        }));
    rocket.launch().await?;

    Ok(())
}
