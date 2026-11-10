//! AudioShare Uploader: uploads programmes from a studio server to AudioShare,
//! at fixed times or when a file is renewed. Runs in the background under the
//! signed-in user (double-click = install), or as a Windows service. Web
//! interface on port 8470.

#![allow(dead_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

mod audioshare;
mod config;
mod decode;
mod desktop;
mod engine;
mod export;
mod media;
mod rules;
mod shares;
mod wav;
mod web;

const HELP: &str = "AudioShare Uploader

Normaal gebruik: dubbelklik op AudioShareUploader.exe. Hij draait dan op de
achtergrond onder deze Windows-gebruiker, start voortaan mee met Windows en
opent de webinterface (http://localhost:8470).

Opdrachten:
  AudioShareUploader run         draaien in dit venster (testen; stoppen met Ctrl+C)
  AudioShareUploader install     als Windows-service installeren (als administrator; alleen
                                 nodig als de pc niet ingelogd blijft)
  AudioShareUploader uninstall   die service weer verwijderen (als administrator)

Gegevens staan in C:\\ProgramData\\AudioShareUploader\\";

fn run_foreground() -> Result<(), String> {
    let app = engine::App::start();
    let port = app.settings.lock().unwrap().port;
    println!("AudioShare Uploader {} draait. Webinterface: http://localhost:{port}  (Ctrl+C = stoppen)", env!("CARGO_PKG_VERSION"));
    web::serve(app)
}

/// Started by autostart (or by a double-click): engine + web, no window.
fn run_background() -> Result<(), String> {
    let port = config::Settings::load().port;
    if desktop::port_in_use(port) {
        return Ok(()); // already running
    }
    let app = engine::App::start();
    web::serve(app)
}

/// Double-click: install for this user and open the web interface.
fn double_click() -> Result<(), String> {
    let port = config::Settings::load().port;
    let url = format!("http://localhost:{port}");
    if desktop::service_installed() {
        desktop::message(
            "AudioShare Uploader",
            "Op deze pc staat nog de service-versie van de AudioShare Uploader.\n\nVerwijder die eerst: open een opdrachtprompt als administrator, ga naar deze map en typ:\n\nAudioShareUploader.exe uninstall\n\nDubbelklik daarna opnieuw. Taken en koppeling blijven bewaard.",
            true,
        );
        return Ok(());
    }
    if desktop::port_in_use(port) {
        desktop::open_browser(&url);
        return Ok(());
    }
    let mut s = config::Settings::load();
    if s.autostart {
        desktop::set_autostart(true)?;
    }
    if !s.installed_once {
        s.installed_once = true;
        s.save();
    }
    desktop::spawn_background()?;
    std::thread::sleep(std::time::Duration::from_millis(1500));
    desktop::open_browser(&url);
    Ok(())
}

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    if !matches!(cmd.as_str(), "" | "background" | "service") {
        desktop::attach_console();
    }
    let res = match cmd.as_str() {
        "run" => run_foreground(),
        "background" => run_background(),
        #[cfg(windows)]
        "" => double_click(),
        #[cfg(windows)]
        "install" => service::install(),
        #[cfg(windows)]
        "uninstall" => service::uninstall(),
        #[cfg(windows)]
        "service" => service::dispatch(),
        _ => {
            println!("{HELP}");
            Ok(())
        }
    };
    if let Err(e) = res {
        eprintln!("Fout: {e}");
        if cmd.is_empty() {
            desktop::message("AudioShare Uploader", &e, true);
        }
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod service {
    use std::ffi::OsString;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState,
        ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    const NAME: &str = "AudioShareUploader";
    const DISPLAY: &str = "AudioShare Uploader";

    define_windows_service!(ffi_service_main, service_main);

    pub fn dispatch() -> Result<(), String> {
        service_dispatcher::start(NAME, ffi_service_main).map_err(|e| format!("Kan niet als service starten: {e}. Gebruik 'install' of 'run'."))
    }

    fn status(state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running { ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN } else { ServiceControlAccept::empty() },
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        }
    }

    fn service_main(_args: Vec<OsString>) {
        crate::engine::AS_SERVICE.store(true, Ordering::Relaxed);
        let app = crate::engine::App::start();
        let a = app.clone();
        let handler = move |c| match c {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                a.stop.store(true, Ordering::Relaxed);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let Ok(h) = service_control_handler::register(NAME, handler) else { return };
        let _ = h.set_service_status(status(ServiceState::Running));
        if let Err(e) = crate::web::serve(app.clone()) {
            app.log("systeem", crate::audioshare::queue::Level::Error, format!("Webinterface niet gestart: {e}"));
            // Uploading goes on without the web interface.
            while !app.stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        let _ = h.set_service_status(status(ServiceState::Stopped));
    }

    pub fn install() -> Result<(), String> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
            .map_err(|e| format!("Geen toegang tot Windows-services (start als administrator): {e}"))?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let info = ServiceInfo {
            name: NAME.into(),
            display_name: DISPLAY.into(),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe,
            launch_arguments: vec!["service".into()],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        };
        let svc = mgr
            .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
            .map_err(|e| format!("Service aanmaken mislukt (bestaat hij al? gebruik eerst 'uninstall'): {e}"))?;
        let _ = svc.set_description("Uploadt programma's automatisch naar AudioShare. Instellen via http://localhost:8470");
        svc.start(&[] as &[&std::ffi::OsStr]).map_err(|e| format!("Service starten mislukt: {e}"))?;
        println!("Service \"{DISPLAY}\" geïnstalleerd en gestart.\nOpen nu http://localhost:8470 op deze pc.");
        Ok(())
    }

    pub fn uninstall() -> Result<(), String> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(|e| format!("Geen toegang tot Windows-services (start als administrator): {e}"))?;
        let svc = mgr
            .open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)
            .map_err(|e| format!("Service niet gevonden: {e}"))?;
        if svc.query_status().map(|s| s.current_state != ServiceState::Stopped).unwrap_or(false) {
            let _ = svc.stop();
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(500));
                if svc.query_status().map(|s| s.current_state == ServiceState::Stopped).unwrap_or(true) {
                    break;
                }
            }
        }
        svc.delete().map_err(|e| format!("Verwijderen mislukt: {e}"))?;
        println!("Service \"{DISPLAY}\" verwijderd. Instellingen en logboek blijven staan in C:\\ProgramData\\AudioShareUploader\\.");
        Ok(())
    }
}
