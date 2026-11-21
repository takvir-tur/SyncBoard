use std::sync::{Arc, OnceLock};

use clipsync_core::protocol::ClipKind;
use clipsync_core::Engine;
use qrcode::{Color, QrCode};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    COLOR_WINDOW, HBRUSH, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowTextLengthW, GetWindowTextW,
    LoadCursorW, MessageBoxW, RegisterClassW, SetForegroundWindow, SetWindowTextW, ShowWindow,
    BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, ES_AUTOHSCROLL, ES_READONLY, HMENU,
    IDC_ARROW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, SW_SHOW, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_DESTROY, WM_PAINT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_SYSMENU,
    WS_VISIBLE,
};

use crate::clipboard::ClipboardFormats;

const ID_URI_EDIT: usize = 101;
const ID_COPY_BTN: usize = 102;
const ID_JOIN_EDIT: usize = 103;
const ID_JOIN_BTN: usize = 104;

struct QrWindowState {
    engine: Arc<Engine>,
    formats: ClipboardFormats,
    uri_edit: HWND,
    join_edit: HWND,
}

static QR_HWND: OnceLock<std::sync::Mutex<Option<isize>>> = OnceLock::new();
static REGISTERED: OnceLock<()> = OnceLock::new();

fn qr_hwnd_cell() -> &'static std::sync::Mutex<Option<isize>> {
    QR_HWND.get_or_init(|| std::sync::Mutex::new(None))
}

pub fn show_qr_window(engine: Arc<Engine>) {
    unsafe {
        if let Some(raw) = *qr_hwnd_cell().lock().unwrap() {
            let hwnd = HWND(raw as *mut core::ffi::c_void);
            if !hwnd.is_invalid() {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
                let _ = InvalidateRect(hwnd, None, true);
                return;
            }
        }

        let Ok(hmod) = GetModuleHandleW(None) else {
            return;
        };
        let hinst: HINSTANCE = hmod.into();
        let class_name = w!("ClipSyncQrWindow");

        REGISTERED.get_or_init(|| {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(qr_wnd_proc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut core::ffi::c_void),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassW(&wc);
        });

        let state = Box::new(QrWindowState {
            engine,
            formats: ClipboardFormats::register(),
            uri_edit: HWND::default(),
            join_edit: HWND::default(),
        });

        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("ClipSync - Pair Device"),
            WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            396,
            510,
            None,
            None,
            hinst,
            Some(Box::into_raw(state) as *const core::ffi::c_void),
        );

        if let Ok(hwnd) = hwnd {
            *qr_hwnd_cell().lock().unwrap() = Some(hwnd.0 as isize);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

unsafe extern "system" fn qr_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            let state_ptr = cs.lpCreateParams as *mut QrWindowState;
            windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                state_ptr as isize,
            );

            let state = &mut *state_ptr;
            let hinst = cs.hInstance;
            let uri_wide = to_wide(&state.engine.get_pairing_uri());

            state.uri_edit = CreateWindowExW(
                Default::default(),
                w!("EDIT"),
                PCWSTR(uri_wide.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_BORDER | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(ES_READONLY as u32 | ES_AUTOHSCROLL as u32),
                20,
                340,
                250,
                26,
                hwnd,
                HMENU(ID_URI_EDIT as *mut core::ffi::c_void),
                hinst,
                None,
            )
            .unwrap_or_default();

            let _ = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Copy URI"),
                WS_CHILD | WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_PUSHBUTTON as u32),
                278,
                340,
                88,
                26,
                hwnd,
                HMENU(ID_COPY_BTN as *mut core::ffi::c_void),
                hinst,
                None,
            );

            state.join_edit = CreateWindowExW(
                Default::default(),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_BORDER | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                20,
                382,
                250,
                26,
                hwnd,
                HMENU(ID_JOIN_EDIT as *mut core::ffi::c_void),
                hinst,
                None,
            )
            .unwrap_or_default();

            let _ = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                w!("Join Cluster"),
                WS_CHILD | WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_PUSHBUTTON as u32),
                278,
                382,
                88,
                26,
                hwnd,
                HMENU(ID_JOIN_BTN as *mut core::ffi::c_void),
                hinst,
                None,
            );

            LRESULT(0)
        }
        WM_PAINT => {
            let state_ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            ) as *mut QrWindowState;

            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !state_ptr.is_null() {
                let state = &*state_ptr;
                let uri = state.engine.get_pairing_uri();
                if let Ok(code) = QrCode::new(uri.as_bytes()) {
                    let width = code.width() as i32;
                    let target_px = 300;
                    let module = (target_px / (width + 4)).max(3);
                    let quiet = 2;
                    let total_px = (width + quiet * 2) * module;
                    let origin_x = (380 - total_px) / 2;
                    let origin_y = 20;

                    let white = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
                    let black = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x0000_0000));

                    let bg_rect = RECT {
                        left: origin_x,
                        top: origin_y,
                        right: origin_x + total_px,
                        bottom: origin_y + total_px,
                    };
                    FillRect(hdc, &bg_rect, white);

                    for y in 0..width {
                        for x in 0..width {
                            if code[(x as usize, y as usize)] == Color::Dark {
                                let r = RECT {
                                    left: origin_x + (x + quiet) * module,
                                    top: origin_y + (y + quiet) * module,
                                    right: origin_x + (x + quiet + 1) * module,
                                    bottom: origin_y + (y + quiet + 1) * module,
                                };
                                FillRect(hdc, &r, black);
                            }
                        }
                    }

                    let _ = DeleteObject(white);
                    let _ = DeleteObject(black);
                }
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_COMMAND => {
            let control_id = (wparam.0 & 0xFFFF) as usize;
            let state_ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            ) as *mut QrWindowState;
            if !state_ptr.is_null() {
                let state = &*state_ptr;
                if control_id == ID_COPY_BTN {
                    let uri = state.engine.get_pairing_uri();
                    state.engine.record_echo(ClipKind::Text, uri.as_bytes());
                    state.formats.write(hwnd, ClipKind::Text, uri.as_bytes());
                } else if control_id == ID_JOIN_BTN {
                    let text = read_edit_text(state.join_edit);
                    match state.engine.join_pairing_uri(&text) {
                        Ok(new_uri) => {
                            let wide = to_wide(&new_uri);
                            let _ = SetWindowTextW(state.uri_edit, PCWSTR(wide.as_ptr()));
                            let _ = SetWindowTextW(state.join_edit, w!(""));
                            let _ = InvalidateRect(hwnd, None, true);
                            MessageBoxW(
                                hwnd,
                                w!("Joined new ClipSync cluster."),
                                w!("ClipSync"),
                                MB_OK | MB_ICONINFORMATION,
                            );
                        }
                        Err(_) => {
                            MessageBoxW(
                                hwnd,
                                w!("Invalid clipsync://pair URI."),
                                w!("ClipSync"),
                                MB_OK | MB_ICONERROR,
                            );
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let state_ptr = windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                0,
            ) as *mut QrWindowState;
            if !state_ptr.is_null() {
                drop(Box::from_raw(state_ptr));
            }
            *qr_hwnd_cell().lock().unwrap() = None;
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

unsafe fn read_edit_text(edit: HWND) -> String {
    let len = GetWindowTextLengthW(edit);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let copied = GetWindowTextW(edit, &mut buf);
    String::from_utf16_lossy(&buf[..copied as usize])
}
