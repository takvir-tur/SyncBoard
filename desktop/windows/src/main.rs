#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod clipboard;
mod qr_window;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use clipsync_core::protocol::ClipKind;
use clipsync_core::Engine;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, HWND_MESSAGE, MSG, WM_APP, WM_CLIPBOARDUPDATE, WM_CREATE,
    WM_DESTROY, WNDCLASSW,
};

use clipboard::{ClipboardContent, ClipboardFormats};

const DEFAULT_RELAY_URL: &str = "wss://clipsync-relay.takvir-tur.workers.dev";

const WM_APP_SYNC_EVENT: u32 = WM_APP + 1;
const WM_APP_MENU_EVENT: u32 = WM_APP + 2;

const MENU_PIN_CURRENT: &str = "pin_current";
const MENU_PAIR_QR: &str = "pair_qr";
const MENU_AUTOSTART: &str = "autostart";
const MENU_QUIT: &str = "quit";

enum MenuAction {
    PinCurrent,
    PairQr,
    ToggleAutostart,
    Quit,
    CopyPin([u8; 16]),
    Unpin([u8; 16]),
}

struct AppState {
    hwnd: HWND,
    engine: Arc<Engine>,
    formats: ClipboardFormats,
    tray: TrayIcon,
    actions: HashMap<MenuId, MenuAction>,
}

static PENDING_MENU_IDS: OnceLock<Mutex<Vec<MenuId>>> = OnceLock::new();

fn pending_menu_queue() -> &'static Mutex<Vec<MenuId>> {
    PENDING_MENU_IDS.get_or_init(|| Mutex::new(Vec::new()))
}

fn app_data_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(appdata).join("ClipSync")
    } else {
        std::env::temp_dir().join("ClipSync")
    }
}

fn create_tray_icon_graphic() -> Icon {
    let size = 32usize;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            let in_board = (6..26).contains(&x) && (5..28).contains(&y);
            let in_clip = (11..21).contains(&x) && (2..7).contains(&y);
            let in_lines = (10..22).contains(&x) && matches!(y, 12 | 17 | 22);
            if in_clip {
                rgba[idx] = 255;
                rgba[idx + 1] = 255;
                rgba[idx + 2] = 255;
                rgba[idx + 3] = 255;
            } else if in_board && !in_lines {
                rgba[idx] = 56;
                rgba[idx + 1] = 142;
                rgba[idx + 2] = 247;
                rgba[idx + 3] = 255;
            } else if in_lines {
                rgba[idx] = 255;
                rgba[idx + 1] = 255;
                rgba[idx + 2] = 255;
                rgba[idx + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, size as u32, size as u32).expect("valid 32x32 icon")
}

fn build_tray_menu(engine: &Engine) -> (Menu, HashMap<MenuId, MenuAction>) {
    let menu = Menu::new();
    let mut actions = HashMap::new();

    let pin_now = MenuItem::with_id(MENU_PIN_CURRENT, "Pin Current Clipboard", true, None);
    actions.insert(pin_now.id().clone(), MenuAction::PinCurrent);
    let _ = menu.append(&pin_now);

    let pins = engine.get_pins();
    let pins_sub = Submenu::new(format!("Pinned Items ({})", pins.len()), true);
    let unpin_sub = Submenu::new("Unpin Item", !pins.is_empty());

    if pins.is_empty() {
        let empty = MenuItem::new("(No pinned items)", false, None);
        let _ = pins_sub.append(&empty);
    } else {
        for pin in pins {
            let label = match pin.kind {
                ClipKind::Text => {
                    let s = String::from_utf8_lossy(&pin.data);
                    let clean: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).take(42).collect();
                    if s.chars().count() > 42 {
                        format!("{clean}...")
                    } else {
                        clean
                    }
                }
                ClipKind::Image => format!("[Image: {} ({} KB)]", pin.mime, (pin.data.len() + 1023) / 1024),
            };
            let copy_item = MenuItem::new(&label, true, None);
            actions.insert(copy_item.id().clone(), MenuAction::CopyPin(pin.id));
            let _ = pins_sub.append(&copy_item);

            let del_item = MenuItem::new(format!("Remove: {label}"), true, None);
            actions.insert(del_item.id().clone(), MenuAction::Unpin(pin.id));
            let _ = unpin_sub.append(&del_item);
        }
    }

    let _ = menu.append(&pins_sub);
    let _ = menu.append(&unpin_sub);
    let _ = menu.append(&PredefinedMenuItem::separator());

    let pair_qr = MenuItem::with_id(MENU_PAIR_QR, "Pair Device (QR Code)...", true, None);
    actions.insert(pair_qr.id().clone(), MenuAction::PairQr);
    let _ = menu.append(&pair_qr);

    let autostart_item = CheckMenuItem::with_id(
        MENU_AUTOSTART,
        "Start at Login",
        true,
        autostart::is_enabled(),
        None,
    );
    actions.insert(autostart_item.id().clone(), MenuAction::ToggleAutostart);
    let _ = menu.append(&autostart_item);

    let _ = menu.append(&PredefinedMenuItem::separator());

    let quit = MenuItem::with_id(MENU_QUIT, "Quit ClipSync", true, None);
    actions.insert(quit.id().clone(), MenuAction::Quit);
    let _ = menu.append(&quit);

    (menu, actions)
}

fn main() {
    let data_dir = app_data_dir();
    autostart::ensure_default_enabled(&data_dir);

    let relay_override = std::env::var("CLIPSYNC_RELAY_URL").ok();
    let engine = Arc::new(
        Engine::new(
            &data_dir,
            Some(relay_override.as_deref().unwrap_or(DEFAULT_RELAY_URL)),
        )
        .expect("init ClipSync engine"),
    );

    unsafe {
        let hmod = GetModuleHandleW(None).expect("GetModuleHandleW");
        let hinst: HINSTANCE = hmod.into();
        let class_name = w!("ClipSyncHiddenClipboardWnd");

        let wc = WNDCLASSW {
            lpfnWndProc: Some(hidden_wnd_proc),
            hInstance: hinst,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("ClipSync"),
            Default::default(),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            hinst,
            None,
        )
        .expect("CreateWindowExW HWND_MESSAGE");

        let formats = ClipboardFormats::register();
        let (menu, actions) = build_tray_menu(&engine);
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ClipSync — Encrypted Shared Clipboard")
            .with_icon(create_tray_icon_graphic())
            .build()
            .expect("build tray icon");

        let hwnd_raw = hwnd.0 as isize;
        engine.set_event_callback(Some(Box::new(move || {
            let target = HWND(hwnd_raw as *mut core::ffi::c_void);
            let _ = PostMessageW(target, WM_APP_SYNC_EVENT, WPARAM(0), LPARAM(0));
        })));

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            pending_menu_queue().lock().unwrap().push(event.id);
            let target = HWND(hwnd_raw as *mut core::ffi::c_void);
            let _ = PostMessageW(target, WM_APP_MENU_EVENT, WPARAM(0), LPARAM(0));
        }));

        let state = Box::new(AppState {
            hwnd,
            engine: Arc::clone(&engine),
            formats,
            tray,
            actions,
        });
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            Box::into_raw(state) as isize,
        );

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        engine.shutdown();
    }
}

unsafe extern "system" fn hidden_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
        hwnd,
        windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
    ) as *mut AppState;

    match msg {
        WM_CREATE => {
            let _ = AddClipboardFormatListener(hwnd);
            LRESULT(0)
        }
        WM_CLIPBOARDUPDATE => {
            if !state_ptr.is_null() {
                let state = &*state_ptr;
                if let Some(content) = state.formats.read(hwnd) {
                    match content {
                        ClipboardContent::Text(bytes) => {
                            let _ = state.engine.push_local(ClipKind::Text, &bytes);
                        }
                        ClipboardContent::Image(bytes) => {
                            let _ = state.engine.push_local(ClipKind::Image, &bytes);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_SYNC_EVENT => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                while let Some(remote) = state.engine.pop_remote() {
                    state.engine.record_echo(remote.kind, &remote.data);
                    if let Some(written) = state.formats.write(hwnd, remote.kind, &remote.data) {
                        state.engine.record_echo(remote.kind, &written);
                    }
                }
                refresh_tray_menu(state);
            }
            LRESULT(0)
        }
        WM_APP_MENU_EVENT => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let ids: Vec<MenuId> = {
                    let mut q = pending_menu_queue().lock().unwrap();
                    q.drain(..).collect()
                };
                for id in ids {
                    if let Some(action) = state.actions.get(&id) {
                        match action {
                            MenuAction::PinCurrent => {
                                if let Some(content) = state.formats.read(hwnd) {
                                    match content {
                                        ClipboardContent::Text(bytes) => {
                                            let _ = state.engine.pin_item(ClipKind::Text, &bytes);
                                        }
                                        ClipboardContent::Image(bytes) => {
                                            let _ = state.engine.pin_item(ClipKind::Image, &bytes);
                                        }
                                    }
                                    refresh_tray_menu(state);
                                }
                            }
                            MenuAction::CopyPin(pin_id) => {
                                let pins = state.engine.get_pins();
                                if let Some(pin) = pins.into_iter().find(|p| &p.id == pin_id) {
                                    state.engine.record_echo(pin.kind, &pin.data);
                                    if let Some(written) =
                                        state.formats.write(hwnd, pin.kind, &pin.data)
                                    {
                                        state.engine.record_echo(pin.kind, &written);
                                    }
                                }
                            }
                            MenuAction::Unpin(pin_id) => {
                                let _ = state.engine.unpin_item(pin_id);
                                refresh_tray_menu(state);
                            }
                            MenuAction::PairQr => {
                                qr_window::show_qr_window(Arc::clone(&state.engine));
                            }
                            MenuAction::ToggleAutostart => {
                                let next = !autostart::is_enabled();
                                autostart::set_enabled(next);
                                refresh_tray_menu(state);
                            }
                            MenuAction::Quit => {
                                PostQuitMessage(0);
                            }
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = RemoveClipboardFormatListener(hwnd);
            if !state_ptr.is_null() {
                windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                    hwnd,
                    windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                    0,
                );
                drop(Box::from_raw(state_ptr));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn refresh_tray_menu(state: &mut AppState) {
    let (menu, actions) = build_tray_menu(&state.engine);
    let _ = state.tray.set_menu(Some(Box::new(menu)));
    state.actions = actions;
}
