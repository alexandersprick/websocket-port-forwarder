use crate::{parse_forward_rules, run_client, Settings};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows_service::{
    define_windows_service,
    service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const SERVICE_NAME: &str = "ws-forwarder-client";
const SERVICE_DISPLAY_NAME: &str = "WebSocket Port Forwarder Client";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
const DEFAULT_CONFIG_NAME: &str = "ws-forwarder-client.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    server: String,
    forward: Vec<String>,
    #[serde(default)]
    insecure: bool,
    #[serde(default = "default_retry_interval")]
    retry_interval: u64,
    /// Relative paths are resolved against the config file's directory.
    log_file: Option<PathBuf>,
}

fn default_retry_interval() -> u64 {
    60
}

struct ServiceConfig {
    settings: Settings,
    log_file: Option<PathBuf>,
}

static CONFIG: OnceLock<ServiceConfig> = OnceLock::new();

fn load_config(path: &Path) -> Result<ServiceConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file {}", path.display()))?;
    let config: Config = toml::from_str(&text)
        .with_context(|| format!("Invalid config file {}", path.display()))?;

    parse_forward_rules(&config.forward)?;

    let base = path.parent().unwrap_or(Path::new(""));
    Ok(ServiceConfig {
        settings: Settings {
            server: config.server,
            forward: config.forward,
            insecure: config.insecure,
            retry_interval: config.retry_interval,
        },
        log_file: config.log_file.map(|p| base.join(p)),
    })
}

pub fn install(config: Option<PathBuf>) -> Result<()> {
    let exe = std::env::current_exe()?;
    let config_path = config.unwrap_or_else(|| exe.with_file_name(DEFAULT_CONFIG_NAME));
    let config_path = std::path::absolute(&config_path)?;
    load_config(&config_path)?;

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("Failed to connect to the service manager (run as administrator)")?;

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("--service"), config_path.clone().into_os_string()],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };

    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG)
        .context("Failed to create service")?;
    service.set_description("Reverse tunnel client over WebSocket")?;

    println!(
        "Service '{}' installed with config {}. Start it with: sc start {}",
        SERVICE_NAME,
        config_path.display(),
        SERVICE_NAME
    );
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to connect to the service manager (run as administrator)")?;
    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        )
        .context("Failed to open service")?;

    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop()?;
    }
    service.delete()?;

    println!("Service '{}' uninstalled", SERVICE_NAME);
    Ok(())
}

/// Blocks until the service has stopped. Must be called from a process started by the service manager.
pub fn run(config_path: &Path) -> Result<()> {
    let config = load_config(config_path)?;
    let _ = CONFIG.set(config);
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("Failed to start the service dispatcher (--service is only valid when started by the service manager)")?;
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!("Service failed: {:#}", e);
    }
}

fn run_service() -> Result<()> {
    let config = CONFIG.get().context("Service config not loaded")?;

    if let Some(path) = &config.log_file {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("Failed to open log file {}", path.display()))?;
        tracing_subscriber::fmt()
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .init();
    }

    let runtime = tokio::runtime::Runtime::new()?;
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let stop_tx = Mutex::new(Some(stop_tx));

    let status_handle = service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            if let Some(tx) = stop_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    set_status(
        &status_handle,
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ServiceExitCode::NO_ERROR,
    )?;

    let result = runtime.block_on(async {
        tokio::select! {
            r = run_client(&config.settings) => r,
            _ = stop_rx => Ok(()),
        }
    });

    let exit_code = if result.is_ok() {
        ServiceExitCode::NO_ERROR
    } else {
        ServiceExitCode::ServiceSpecific(1)
    };
    set_status(&status_handle, ServiceState::Stopped, ServiceControlAccept::empty(), exit_code)?;

    result
}

fn set_status(
    handle: &ServiceStatusHandle,
    state: ServiceState,
    controls_accepted: ServiceControlAccept,
    exit_code: ServiceExitCode,
) -> Result<()> {
    handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: state,
        controls_accepted,
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    Ok(())
}
