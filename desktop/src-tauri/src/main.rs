// Card 739: Valnivo Crew as an app for macOS and Windows. One window on https://crew.valnivo.eu/ui/ (the same Console
// as the browser, signed in with the same account), remembered size and place, one copy at a time, and links to anywhere else
// open in the normal browser. The page gets almost no access to the computer: only Valnivo Crew's own site may show a
// notification, put the waiting count on the Dock icon, and install a signed update (capabilities/default.json).
// Nothing else is exposed. The private update key is not in this program.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod control;
mod input_guard;

use tauri::{Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

const HOME: &str = "https://crew.valnivo.eu/ui/";   // Kabil 17 Nov: the crew app moved here; console.valnivo.eu becomes the testing app

/// Pages that stay in the app window: the Console itself and Valnivo's own hosts (the shared desktop view); local schemes
/// the page uses for downloads and previews.
fn stays_inside(url: &Url) -> bool {
    match url.scheme() {
        "about" | "blob" | "data" => true,
        "https" => url.host_str().is_some_and(|h| h == "valnivo.eu" || h.ends_with(".valnivo.eu")),
        _ => false,
    }
}

/// The paths of the control service the page may reach through the app (= the routes in control.rs serve).
const HELPER_PATHS: [&str; 3] = ["/health", "/state", "/act"];

/// Kabil 17 Nov: on macOS the app's web engine (WebKit) blocks an https page from calling http://127.0.0.1 (mixed content), so Give
/// control never reached the built-in helper. Inside the app the page hands the call to the app instead; the app forwards it to its
/// own control service on loopback, unchanged, with the window's real origin (taken from the window, never from the page), so the
/// service applies exactly the same checks (origin, the grant's pass, the action rules) as for a browser.
/// Async on purpose (Kabil 18 Nov, "Give control still has an error"): a sync command runs on the main thread, which is also the thread
/// macOS needs for the helper's mouse and keyboard input, so an /act forwarded from the main thread waited on itself. The blocking HTTP
/// call runs on a worker thread instead.
#[tauri::command]
async fn helper(window: tauri::WebviewWindow, path: String, body: Option<String>) -> Result<(u16, String), String> {
    if !HELPER_PATHS.contains(&path.as_str()) { return Err("not a helper path".into()); }
    if body.as_ref().is_some_and(|b| b.len() > 64 * 1024) { return Err("too large".into()); }
    let url = window.url().map_err(|e| e.to_string())?;
    let origin = match (url.scheme(), url.host_str()) { ("https", Some(h)) => format!("https://{h}"), _ => return Err("not the app's page".into()) };
    tauri::async_runtime::spawn_blocking(move || forward(&origin, &path, body)).await.map_err(|e| e.to_string())?
}

fn forward(origin: &str, path: &str, body: Option<String>) -> Result<(u16, String), String> {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(30)).build();
    let target = format!("http://127.0.0.1:{}{}", control::PORT, path);
    let r = match body {
        Some(b) => agent.post(&target).set("Origin", origin).set("Content-Type", "application/json").send_string(&b),
        None => agent.get(&target).set("Origin", origin).call(),
    };
    match r {
        Ok(resp) => { let code = resp.status(); Ok((code, resp.into_string().unwrap_or_default())) }
        Err(ureq::Error::Status(code, resp)) => Ok((code, resp.into_string().unwrap_or_default())),
        Err(e) => Err(format!("helper unreachable: {e}")),
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![helper])
        // a second launch brings the open window forward instead of starting another copy
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Kabil 17 Nov: Give control is built into the app (control.rs); the separate helper stays the fallback for a browser
            let _ = control::start(app.handle().clone());
            let home: Url = HOME.parse().expect("the Console address");
            let handle = app.handle().clone();
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(home))
                .title("Valnivo Crew")
                .inner_size(1280.0, 820.0)
                .min_inner_size(720.0, 520.0)
                // dark title bar and window background to match the dark Console (#0a0b0d)
                .theme(Some(tauri::Theme::Dark))
                .background_color(tauri::window::Color(10, 11, 13, 255))
                // on Windows Tauri's own drop handler takes file drops away from the page: let the page get them
                .disable_drag_drop_handler()
                .on_navigation(move |url| {
                    if stays_inside(url) {
                        return true;
                    }
                    if matches!(url.scheme(), "https" | "http" | "mailto") {
                        let _ = handle.opener().open_url(url.as_str(), None::<&str>);
                    }
                    false
                })
                .build()?;
            // Theme::Dark does not set the caption color. Windows 11 can still paint a light bar.
            #[cfg(windows)]
            caption::install(&window);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the Valnivo Crew app could not start");
}

/// The DWM call. The colors and the order of attributes live in the library so they can be tested
/// without a Windows machine. A failed call is ignored: the window stays open on the theme already set.
#[cfg(windows)]
mod caption {
    use valnivo_console::caption::{attempts, dwm_accepted, ATTR_IMMERSIVE};

    fn set_dwm(hwnd: isize, attr: u32, value: u32) -> bool {
        #[link(name = "dwmapi")]
        extern "system" {
            fn DwmSetWindowAttribute(
                hwnd: isize,
                dw_attribute: u32,
                pv_attribute: *const std::ffi::c_void,
                cb_attribute: u32,
            ) -> i32;
        }
        let code = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attr,
                &value as *const u32 as *const std::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            )
        };
        dwm_accepted(code)
    }

    fn paint(hwnd: isize) {
        if hwnd == 0 {
            return;
        }
        let immersive_ok = set_dwm(hwnd, ATTR_IMMERSIVE, 1);
        for (attr, value) in attempts(immersive_ok) {
            if attr == ATTR_IMMERSIVE {
                continue;
            }
            let _ = set_dwm(hwnd, attr, value);
        }
    }

    /// Paint once, and again when Windows rebuilds the caption (focus, size, scale).
    pub fn install(window: &tauri::WebviewWindow) {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        paint(hwnd.0 as isize);
        let watch = window.clone();
        window.on_window_event(move |event| {
            // The event list is non_exhaustive, so the last arm has to be a wildcard.
            let again = match event {
                tauri::WindowEvent::Focused(_)
                | tauri::WindowEvent::Resized(_)
                | tauri::WindowEvent::ScaleFactorChanged { .. } => true,
                _ => false,
            };
            if !again {
                return;
            }
            if let Ok(hwnd) = watch.hwnd() {
                paint(hwnd.0 as isize);
            }
        });
    }
}
