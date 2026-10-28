use std::{ffi::OsStr, path::PathBuf};

use doctor_api::{
    AccessConfig, ApiConfig, ApiState, DEFAULT_ADMIN_PASSWORD, DEFAULT_ADMIN_USER,
    DOCKER_LOOPBACK_PUBLISHED_BIND_ADDR, DeploymentMode, serve,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let frontend_dist = std::env::var_os("DOCTOR_FRONTEND_DIST")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("frontend/dist"));
    let mut config = if std::env::var("DOCTOR_DEPLOYMENT_MODE").as_deref() == Ok("network") {
        network_config_from_environment(|name| std::env::var(name).ok(), frontend_dist)?
    } else {
        api_config_from_environment(
            std::env::var_os("DOCTOR_DEPLOYMENT_MODE").as_deref(),
            std::env::var_os("DOCTOR_PUBLIC_PORT").as_deref(),
            std::env::var_os("DOCTOR_PORT").as_deref(),
            frontend_dist,
        )?
    };
    config.history = history_config_from_environment(|name| std::env::var(name).ok())?;
    if config
        .access
        .as_ref()
        .is_some_and(|access| access.password == DEFAULT_ADMIN_PASSWORD)
    {
        eprintln!("login_default_password: set DOCTOR_ADMIN_PASSWORD to change it");
    }
    let state = ApiState::new(config)?;
    serve(state).await?;
    Ok(())
}

/// The history file in the user's data directory. Earlier releases kept it
/// under `starrocks-doctor`; while only that one exists it stays in use, so
/// an upgrade keeps its history.
fn default_history_path(
    base: &std::path::Path,
    exists: impl Fn(&std::path::Path) -> bool,
) -> PathBuf {
    let current = base.join("sr-doctor").join("history.sqlite");
    let earlier = base.join("starrocks-doctor").join("history.sqlite");
    if !exists(&current) && exists(&earlier) {
        earlier
    } else {
        current
    }
}

/// Stage B history: on by default in the user's data directory (never the
/// repository); `DOCTOR_HISTORY=off` turns it off. Remote clusters are only
/// collected with `DOCTOR_COLLECTION_ALLOW_REMOTE=after-g1`.
fn history_config_from_environment(
    var: impl Fn(&str) -> Option<String>,
) -> Result<doctor_api::HistoryConfig, std::io::Error> {
    let invalid = |name: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{name} is out of range"),
        )
    };
    let mut config = doctor_api::HistoryConfig::default();
    if var("DOCTOR_HISTORY").as_deref() == Some("off") {
        return Ok(config);
    }
    config.path = var("DOCTOR_HISTORY_PATH")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let base = var("LOCALAPPDATA")
                .or_else(|| var("XDG_DATA_HOME"))
                .or_else(|| var("HOME").map(|home| format!("{home}/.local/share")))?;
            Some(default_history_path(&PathBuf::from(base), |path| {
                path.exists()
            }))
        });
    let bounded = |name: &str, default: u32, min: u32, max: u32| -> Result<u32, std::io::Error> {
        match var(name) {
            None => Ok(default),
            Some(value) => value
                .parse::<u32>()
                .ok()
                .filter(|value| (min..=max).contains(value))
                .ok_or_else(|| invalid(name)),
        }
    };
    config.period_seconds = bounded(
        "DOCTOR_COLLECTION_PERIOD_SECONDS",
        config.period_seconds,
        5,
        3600,
    )?;
    config.slice_seconds = bounded(
        "DOCTOR_COLLECTION_SLICE_SECONDS",
        config.slice_seconds,
        60,
        3600,
    )?;
    config.lag_seconds = bounded("DOCTOR_COLLECTION_LAG_SECONDS", config.lag_seconds, 5, 3600)?;
    config.initial_backfill_seconds = bounded(
        "DOCTOR_COLLECTION_BACKFILL_SECONDS",
        config.initial_backfill_seconds,
        0,
        24 * 3600,
    )?;
    config.allow_remote_collection =
        var("DOCTOR_COLLECTION_ALLOW_REMOTE").as_deref() == Some("after-g1");
    Ok(config)
}

/// `DOCTOR_DEPLOYMENT_MODE=network`: a server users open as
/// `http://<host>:<port>` from their own browser. It listens on
/// `DOCTOR_LISTEN` (default `0.0.0.0:<DOCTOR_PORT or 8000>`) and asks for the
/// login `DOCTOR_ADMIN_USER` / `DOCTOR_ADMIN_PASSWORD` (default `admin` /
/// `123456`; empty means the default). `DOCTOR_COOKIE_SECURE=on` behind an
/// HTTPS proxy.
fn network_config_from_environment(
    var: impl Fn(&str) -> Option<String>,
    frontend_dist: PathBuf,
) -> Result<ApiConfig, std::io::Error> {
    let invalid =
        |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, message.to_owned());
    let port = parse_port(var("DOCTOR_PORT").as_deref().map(OsStr::new), "DOCTOR_PORT")?;
    let bind_addr = match var("DOCTOR_LISTEN").filter(|value| !value.is_empty()) {
        Some(listen) => listen
            .parse::<std::net::SocketAddr>()
            .map_err(|_| invalid("DOCTOR_LISTEN must be an address and port, e.g. 0.0.0.0:8000"))?,
        None => std::net::SocketAddr::from(([0, 0, 0, 0], port)),
    };
    let setting = |name: &str, default: &str| {
        var(name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default.to_owned())
    };
    Ok(ApiConfig {
        bind_addr,
        deployment_mode: DeploymentMode::Network,
        frontend_dist: Some(frontend_dist),
        access: Some(AccessConfig {
            username: setting("DOCTOR_ADMIN_USER", DEFAULT_ADMIN_USER),
            password: setting("DOCTOR_ADMIN_PASSWORD", DEFAULT_ADMIN_PASSWORD),
            secure_cookie: var("DOCTOR_COOKIE_SECURE").as_deref() == Some("on"),
        }),
        ..ApiConfig::default()
    })
}

fn api_config_from_environment(
    deployment_mode: Option<&OsStr>,
    public_port: Option<&OsStr>,
    local_port: Option<&OsStr>,
    frontend_dist: PathBuf,
) -> Result<ApiConfig, std::io::Error> {
    let mut config = ApiConfig {
        frontend_dist: Some(frontend_dist),
        ..ApiConfig::default()
    };
    match deployment_mode.and_then(OsStr::to_str) {
        None if deployment_mode.is_none() && public_port.is_none() => {
            config
                .bind_addr
                .set_port(parse_port(local_port, "DOCTOR_PORT")?);
            Ok(config)
        }
        Some("network") => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "network mode is configured by network_config_from_environment",
        )),
        Some("docker-loopback-published") => {
            if local_port.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "DOCTOR_PORT is local-only",
                ));
            }
            let public_port = parse_port(public_port, "DOCTOR_PUBLIC_PORT")?;
            config.bind_addr = DOCKER_LOOPBACK_PUBLISHED_BIND_ADDR;
            config.deployment_mode = DeploymentMode::DockerLoopbackPublished { public_port };
            Ok(config)
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "DOCTOR_DEPLOYMENT_MODE must be exactly docker-loopback-published or network",
        )),
    }
}

fn parse_port(value: Option<&OsStr>, name: &str) -> Result<u16, std::io::Error> {
    let Some(value) = value else {
        return Ok(8000);
    };
    let value = value.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{name} must be a decimal port"),
        )
    })?;
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| value.parse::<u16>().ok())
        .flatten()
        .filter(|port| *port > 0)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{name} must be a decimal port from 1 to 65535"),
            )
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_default_history_keeps_an_earlier_file_until_a_current_one_exists() {
        let base = PathBuf::from("data");
        let current = base.join("sr-doctor").join("history.sqlite");
        let earlier = base.join("starrocks-doctor").join("history.sqlite");
        assert_eq!(default_history_path(&base, |_| false), current);
        assert_eq!(default_history_path(&base, |path| path == earlier), earlier);
        assert_eq!(default_history_path(&base, |_| true), current);
    }

    use super::*;

    #[test]
    fn deployment_mode_is_exact_and_public_port_is_docker_only() {
        let local =
            api_config_from_environment(None, None, None, PathBuf::from("frontend/dist")).unwrap();
        assert_eq!(local.bind_addr, doctor_api::DEFAULT_BIND_ADDR);
        assert_eq!(local.deployment_mode, DeploymentMode::Local);

        let docker = api_config_from_environment(
            Some(OsStr::new("docker-loopback-published")),
            Some(OsStr::new("18000")),
            None,
            PathBuf::from("frontend/dist"),
        )
        .unwrap();
        assert_eq!(docker.bind_addr, DOCKER_LOOPBACK_PUBLISHED_BIND_ADDR);
        assert_eq!(
            docker.deployment_mode,
            DeploymentMode::DockerLoopbackPublished { public_port: 18000 }
        );

        for (mode, port) in [
            (Some(OsStr::new("docker")), None),
            (
                Some(OsStr::new("docker-loopback-published")),
                Some(OsStr::new("0")),
            ),
            (None, Some(OsStr::new("8000"))),
        ] {
            assert!(
                api_config_from_environment(mode, port, None, PathBuf::from("frontend/dist"))
                    .is_err()
            );
        }
    }
    #[test]
    fn network_mode_defaults_to_admin_and_listens_where_told() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let defaults = network_config_from_environment(env(&[]), PathBuf::new()).unwrap();
        assert_eq!(defaults.access, Some(AccessConfig::default_admin(false)));
        assert_eq!(defaults.bind_addr.to_string(), "0.0.0.0:8000");
        assert!(format!("{defaults:?}").contains("<redacted>"));
        assert!(!format!("{defaults:?}").contains("123456"));
        let blank = network_config_from_environment(
            env(&[("DOCTOR_ADMIN_USER", " "), ("DOCTOR_ADMIN_PASSWORD", "")]),
            PathBuf::new(),
        )
        .unwrap();
        assert_eq!(blank.access, Some(AccessConfig::default_admin(false)));

        let config = network_config_from_environment(
            env(&[
                ("DOCTOR_ADMIN_USER", "ops"),
                ("DOCTOR_ADMIN_PASSWORD", "  changed-password\n"),
                ("DOCTOR_PORT", "18080"),
            ]),
            PathBuf::new(),
        )
        .unwrap();
        assert_eq!(config.deployment_mode, DeploymentMode::Network);
        assert_eq!(config.bind_addr.to_string(), "0.0.0.0:18080");
        let access = config.access.clone().unwrap();
        assert_eq!(access.username, "ops");
        assert_eq!(access.password, "changed-password");
        assert!(!access.secure_cookie);
        assert!(!format!("{config:?}").contains("changed-password"));
        assert!(ApiState::new(config).is_ok());

        let listen = network_config_from_environment(
            env(&[
                ("DOCTOR_LISTEN", "10.0.0.5:9000"),
                ("DOCTOR_COOKIE_SECURE", "on"),
            ]),
            PathBuf::new(),
        )
        .unwrap();
        assert_eq!(listen.bind_addr.to_string(), "10.0.0.5:9000");
        assert!(listen.access.unwrap().secure_cookie);
        assert!(
            network_config_from_environment(
                env(&[("DOCTOR_LISTEN", "not-an-address")]),
                PathBuf::new()
            )
            .is_err()
        );
    }

    #[test]
    fn local_port_is_strict_decimal_and_loopback_only() {
        for port in ["1", "18054", "65535"] {
            let config =
                api_config_from_environment(None, None, Some(OsStr::new(port)), PathBuf::new())
                    .unwrap();
            assert_eq!(config.bind_addr.ip(), doctor_api::DEFAULT_BIND_ADDR.ip());
            assert_eq!(config.bind_addr.port(), port.parse::<u16>().unwrap());
            assert_eq!(config.deployment_mode, DeploymentMode::Local);
        }
        for port in [
            "",
            "0",
            "65536",
            "+18054",
            "-1",
            " 8000",
            "8000 ",
            "８０００",
            "1.0",
            "localhost:8000",
        ] {
            assert!(
                api_config_from_environment(None, None, Some(OsStr::new(port)), PathBuf::new())
                    .is_err()
            );
        }
        assert!(
            api_config_from_environment(
                Some(OsStr::new("docker-loopback-published")),
                None,
                Some(OsStr::new("18054")),
                PathBuf::new()
            )
            .is_err()
        );
    }
}
