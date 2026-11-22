//! The felucca tray client: saved connections run as in-process agents, a
//! window adds and removes them, and `felucca://` links open a confirmation.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod connections;
mod store;
mod tray;

use std::time::Duration;

use connections::{Connections, Reply, Status, View};
use felucca_i18n::Language;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_deep_link::DeepLinkExt;

/// Started by the login item: stay in the tray instead of opening the window.
const HIDDEN: &str = "--hidden";

struct App {
    connections: Mutex<Connections>,
    /// Links that arrived before the window could listen for them.
    pending: Mutex<Vec<String>>,
}

#[derive(Serialize)]
struct Described {
    server: String,
    device: String,
}

#[tauri::command]
fn list(app: State<'_, App>) -> Vec<View> {
    app.connections.lock().views()
}

#[tauri::command]
fn language(app: State<'_, App>) -> &'static str {
    app.connections.lock().language().code()
}

#[tauri::command]
fn describe(app: State<'_, App>, text: String) -> Result<Described, Reply> {
    let language = app.connections.lock().language();
    let uri = connections::find_uri(&text).ok_or(Reply::Key("error.noLink"))?;
    let (server, device) = connections::describe(uri, language)?;
    Ok(Described { server, device })
}

#[tauri::command]
fn add(app: State<'_, App>, text: String, name: String) -> Result<(), Reply> {
    app.connections.lock().add(&text, &name)
}

#[tauri::command]
fn remove(app: State<'_, App>, id: String) -> Result<(), Reply> {
    app.connections.lock().remove(&id)
}

#[tauri::command]
fn set_enabled(app: State<'_, App>, id: String, enabled: bool) -> Result<(), Reply> {
    app.connections.lock().set_enabled(&id, enabled)
}

#[tauri::command]
fn take_links(app: State<'_, App>) -> Vec<String> {
    std::mem::take(&mut *app.pending.lock())
}

#[tauri::command]
fn autostart(handle: AppHandle) -> Result<bool, Reply> {
    handle
        .autolaunch()
        .is_enabled()
        .map_err(|error| Reply::Text(error.to_string()))
}

#[tauri::command]
fn set_autostart(handle: AppHandle, enabled: bool) -> Result<(), Reply> {
    let manager = handle.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| Reply::Text(error.to_string()))
}

fn show(handle: &AppHandle) {
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Never adds a connection by itself: a web page can open a link, so the user
/// confirms the server and device in the window first.
fn receive_links(handle: &AppHandle, urls: impl IntoIterator<Item = String>) {
    let links: Vec<String> = urls
        .into_iter()
        .filter(|url| felucca_core::config::launch::is_launch_uri(url))
        .collect();
    if links.is_empty() {
        return;
    }
    handle.state::<App>().pending.lock().extend(links);
    show(handle);
    let _ = handle.emit("links", ());
}

fn system_language() -> Language {
    std::env::var("FELUCCA_LANG")
        .ok()
        .as_deref()
        .and_then(Language::parse)
        .or_else(|| sys_locale::get_locales().find_map(|tag| Language::parse(&tag)))
        .unwrap_or(Language::English)
}

/// A windowed program has no terminal, so its log goes to a file that each
/// start replaces. Connection keys are never logged.
fn init_logging(handle: &AppHandle) {
    let file = handle.path().app_log_dir().ok().and_then(|directory| {
        std::fs::create_dir_all(&directory).ok()?;
        std::fs::File::create(directory.join("felucca-desktop.log")).ok()
    });
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("felucca=info,felucca_desktop=info")
    });
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    let _ = match file {
        Some(file) => builder
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .try_init(),
        None => builder.try_init(),
    };
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    init_logging(&handle);
    let language = system_language();
    // A portable copy or a probe keeps its connections beside it instead.
    let directory = match std::env::var_os("FELUCCA_DESKTOP_DIR") {
        Some(directory) => directory.into(),
        None => handle.path().app_config_dir()?,
    };
    let store = store::Store::new(directory);
    let connections = Connections::open(store, language)?;
    app.manage(App {
        connections: Mutex::new(connections),
        pending: Mutex::new(Vec::new()),
    });

    // A tray client belongs in the menu bar, not the Dock.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    // Installers register the scheme; a development or portable copy registers
    // itself so that links reach the binary that is actually running.
    #[cfg(any(windows, target_os = "linux"))]
    if let Err(error) = app.deep_link().register_all() {
        tracing::warn!(%error, "cannot register felucca:// links");
    }
    let opened = app
        .deep_link()
        .get_current()
        .ok()
        .flatten()
        .unwrap_or_default();
    receive_links(&handle, opened.into_iter().map(String::from));
    let links = handle.clone();
    app.deep_link().on_open_url(move |event| {
        receive_links(&links, event.urls().into_iter().map(String::from));
    });

    let labels = tray::labels(language);
    let summary = MenuItem::with_id(
        app,
        "summary",
        tray::summary(language, 0, 0),
        false,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[
            &summary,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "open", labels.open, true, None::<&str>)?,
            &MenuItem::with_id(app, "quit", labels.quit, true, None::<&str>)?,
        ],
    )?;
    let tray = TrayIconBuilder::with_id("felucca")
        .icon(tray_icon())
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Felucca")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|handle, event| match event.id().as_ref() {
            "open" => show(handle),
            "quit" => handle.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Windows convention: a left click opens the window, a right click the menu.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        })
        .build(app)?;

    // Reflect connection state in the tray; the window polls on its own.
    let watcher = handle.clone();
    tauri::async_runtime::spawn(async move {
        let mut last = None;
        loop {
            let views = watcher.state::<App>().connections.lock().views();
            let enabled = views.iter().filter(|view| view.enabled).count();
            let online = views
                .iter()
                .filter(|view| view.status == Status::Online)
                .count();
            if last != Some((online, enabled)) {
                last = Some((online, enabled));
                let text = tray::summary(language, online, enabled);
                let _ = summary.set_text(&text);
                let _ = tray.set_tooltip(Some(format!("Felucca · {text}")));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    if !std::env::args().any(|arg| arg == HIDDEN) {
        show(&handle);
    }
    Ok(())
}

fn tray_icon() -> tauri::image::Image<'static> {
    #[cfg(target_os = "macos")]
    let bytes = include_bytes!("../icons/tray-template.png");
    #[cfg(not(target_os = "macos"))]
    let bytes = include_bytes!("../icons/tray.png");
    tauri::image::Image::from_bytes(bytes).expect("the bundled tray icon is a PNG")
}

fn main() {
    let app = tauri::Builder::default()
        // First: a second launch, including one started by a link, hands its
        // arguments to the running copy and exits.
        .plugin(tauri_plugin_single_instance::init(|handle, _args, _cwd| {
            show(handle);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args([HIDDEN])
                .app_name("Felucca")
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            list,
            language,
            describe,
            add,
            remove,
            set_enabled,
            take_links,
            autostart,
            set_autostart,
        ])
        .setup(setup)
        .build(tauri::generate_context!())
        .expect("the desktop client could not start");
    app.run(|handle, event| match event {
        // Closing the window keeps the connections running in the tray.
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
            api.prevent_close();
            if let Some(window) = handle.get_webview_window("main") {
                let _ = window.hide();
            }
        }
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(app) = handle.try_state::<App>() {
                app.connections.lock().stop_all();
            }
        }
        _ => {}
    });
}
