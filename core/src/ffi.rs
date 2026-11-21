use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::slice;

use crate::protocol::ClipKind;
use crate::Engine;

pub struct ClipSyncEngine {
    inner: Engine,
}

#[repr(C)]
pub struct ClipSyncItem {
    pub kind: u8,
    pub data: *const u8,
    pub data_len: usize,
    pub mime: *mut c_char,
    pub timestamp_ms: u64,
}

#[repr(C)]
pub struct ClipSyncPin {
    pub id: [u8; 16],
    pub kind: u8,
    pub data: *const u8,
    pub data_len: usize,
    pub mime: *mut c_char,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[repr(C)]
pub struct ClipSyncPinList {
    pub items: *mut ClipSyncPin,
    pub count: usize,
}

fn alloc_byte_slice(bytes: Vec<u8>) -> (*const u8, usize) {
    let len = bytes.len();
    if len == 0 {
        return (ptr::null(), 0);
    }
    let boxed: Box<[u8]> = bytes.into_boxed_slice();
    (Box::into_raw(boxed) as *const u8, len)
}

unsafe fn free_byte_slice(ptr: *const u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        let slice_ptr = ptr::slice_from_raw_parts_mut(ptr as *mut u8, len);
        let _ = Box::from_raw(slice_ptr);
    }
}

fn alloc_c_string(s: &str) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| CString::new("").unwrap())
        .into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_init(
    data_dir: *const c_char,
    relay_url: *const c_char,
) -> *mut ClipSyncEngine {
    if data_dir.is_null() {
        return ptr::null_mut();
    }
    let Ok(dir_str) = CStr::from_ptr(data_dir).to_str() else {
        return ptr::null_mut();
    };
    let relay = if relay_url.is_null() {
        None
    } else {
        CStr::from_ptr(relay_url).to_str().ok()
    };

    match Engine::new(dir_str, relay) {
        Ok(inner) => Box::into_raw(Box::new(ClipSyncEngine { inner })),
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_push_local(
    engine: *mut ClipSyncEngine,
    kind: u8,
    data: *const u8,
    len: usize,
) -> i32 {
    if engine.is_null() || data.is_null() || len == 0 {
        return -1;
    }
    let Some(kind) = ClipKind::from_u8(kind) else {
        return -1;
    };
    let slice = slice::from_raw_parts(data, len);
    match (&*engine).inner.push_local(kind, slice) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_pop_remote(
    engine: *mut ClipSyncEngine,
    out_item: *mut ClipSyncItem,
) -> i32 {
    if engine.is_null() || out_item.is_null() {
        return -1;
    }
    let Some(clip) = (&*engine).inner.pop_remote() else {
        return 0;
    };
    let (data, data_len) = alloc_byte_slice(clip.data);
    *out_item = ClipSyncItem {
        kind: clip.kind as u8,
        data,
        data_len,
        mime: alloc_c_string(&clip.mime),
        timestamp_ms: clip.timestamp_ms,
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_pin_item(
    engine: *mut ClipSyncEngine,
    kind: u8,
    data: *const u8,
    len: usize,
    out_id_16: *mut u8,
) -> i32 {
    if engine.is_null() || data.is_null() || len == 0 {
        return -1;
    }
    let Some(kind) = ClipKind::from_u8(kind) else {
        return -1;
    };
    let slice = slice::from_raw_parts(data, len);
    match (&*engine).inner.pin_item(kind, slice) {
        Ok(pin) => {
            if !out_id_16.is_null() {
                ptr::copy_nonoverlapping(pin.id.as_ptr(), out_id_16, 16);
            }
            0
        }
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_unpin_item(
    engine: *mut ClipSyncEngine,
    id_16: *const u8,
) -> i32 {
    if engine.is_null() || id_16.is_null() {
        return -1;
    }
    let mut id = [0u8; 16];
    ptr::copy_nonoverlapping(id_16, id.as_mut_ptr(), 16);
    match (&*engine).inner.unpin_item(&id) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_get_pins(
    engine: *mut ClipSyncEngine,
    out_list: *mut ClipSyncPinList,
) -> i32 {
    if engine.is_null() || out_list.is_null() {
        return -1;
    }
    let pins = (&*engine).inner.get_pins();
    if pins.is_empty() {
        *out_list = ClipSyncPinList {
            items: ptr::null_mut(),
            count: 0,
        };
        return 0;
    }

    let c_pins: Vec<ClipSyncPin> = pins
        .into_iter()
        .map(|p| {
            let (data, data_len) = alloc_byte_slice(p.data);
            ClipSyncPin {
                id: p.id,
                kind: p.kind as u8,
                data,
                data_len,
                mime: alloc_c_string(&p.mime),
                created_at_ms: p.created_at_ms,
                updated_at_ms: p.updated_at_ms,
            }
        })
        .collect();

    let count = c_pins.len();
    let boxed: Box<[ClipSyncPin]> = c_pins.into_boxed_slice();
    *out_list = ClipSyncPinList {
        items: Box::into_raw(boxed) as *mut ClipSyncPin,
        count,
    };
    0
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_get_pairing_uri(engine: *mut ClipSyncEngine) -> *mut c_char {
    if engine.is_null() {
        return ptr::null_mut();
    }
    let uri = (&*engine).inner.get_pairing_uri();
    alloc_c_string(&uri)
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_join_pairing_uri(
    engine: *mut ClipSyncEngine,
    uri: *const c_char,
) -> i32 {
    if engine.is_null() || uri.is_null() {
        return -1;
    }
    let Ok(uri_str) = CStr::from_ptr(uri).to_str() else {
        return -1;
    };
    match (&*engine).inner.join_pairing_uri(uri_str) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_record_echo(
    engine: *mut ClipSyncEngine,
    kind: u8,
    data: *const u8,
    len: usize,
) {
    if engine.is_null() || data.is_null() || len == 0 {
        return;
    }
    if let Some(kind) = ClipKind::from_u8(kind) {
        let slice = slice::from_raw_parts(data, len);
        (&*engine).inner.record_echo(kind, slice);
    }
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_wait_event(
    engine: *mut ClipSyncEngine,
    timeout_ms: u64,
) -> i32 {
    if engine.is_null() {
        return 0;
    }
    if (&*engine).inner.wait_event(timeout_ms) {
        1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_free_item(item: *mut ClipSyncItem) {
    if item.is_null() {
        return;
    }
    let item_ref = &mut *item;
    free_byte_slice(item_ref.data, item_ref.data_len);
    if !item_ref.mime.is_null() {
        let _ = CString::from_raw(item_ref.mime);
    }
    item_ref.data = ptr::null();
    item_ref.data_len = 0;
    item_ref.mime = ptr::null_mut();
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_free_pin_list(list: *mut ClipSyncPinList) {
    if list.is_null() {
        return;
    }
    let list_ref = &mut *list;
    if !list_ref.items.is_null() && list_ref.count > 0 {
        let slice_ptr = ptr::slice_from_raw_parts_mut(list_ref.items, list_ref.count);
        let boxed = Box::from_raw(slice_ptr);
        for pin in boxed.iter() {
            free_byte_slice(pin.data, pin.data_len);
            if !pin.mime.is_null() {
                let _ = CString::from_raw(pin.mime);
            }
        }
    }
    list_ref.items = ptr::null_mut();
    list_ref.count = 0;
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}

#[no_mangle]
pub unsafe extern "C" fn clipsync_shutdown(engine: *mut ClipSyncEngine) {
    if !engine.is_null() {
        let boxed = Box::from_raw(engine);
        boxed.inner.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn c_abi_full_lifecycle() {
        let dir = std::env::temp_dir().join(format!("clipsync_ffi_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let c_dir = CString::new(dir.to_str().unwrap()).unwrap();
        let c_relay = CString::new("wss://127.0.0.1:9999").unwrap();

        unsafe {
            let engine = clipsync_init(c_dir.as_ptr(), c_relay.as_ptr());
            assert!(!engine.is_null());

            let uri_ptr = clipsync_get_pairing_uri(engine);
            assert!(!uri_ptr.is_null());
            let uri = CStr::from_ptr(uri_ptr).to_str().unwrap().to_owned();
            assert!(uri.starts_with("clipsync://pair?k="));
            clipsync_free_string(uri_ptr);

            let text = b"pinned via FFI";
            let mut pin_id = [0u8; 16];
            assert_eq!(
                clipsync_pin_item(engine, 1, text.as_ptr(), text.len(), pin_id.as_mut_ptr()),
                0
            );

            let mut list = ClipSyncPinList {
                items: ptr::null_mut(),
                count: 0,
            };
            assert_eq!(clipsync_get_pins(engine, &mut list), 0);
            assert_eq!(list.count, 1);
            let first = &*list.items;
            assert_eq!(first.id, pin_id);
            assert_eq!(
                slice::from_raw_parts(first.data, first.data_len),
                text
            );
            clipsync_free_pin_list(&mut list);

            assert_eq!(clipsync_unpin_item(engine, pin_id.as_ptr()), 1);

            let new_uri = CString::new(
                "clipsync://pair?k=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef&r=wss%3A%2F%2F127.0.0.1%3A9998",
            )
            .unwrap();
            assert_eq!(clipsync_join_pairing_uri(engine, new_uri.as_ptr()), 0);

            clipsync_shutdown(engine);
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
