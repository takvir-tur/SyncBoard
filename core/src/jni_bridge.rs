use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jint, jlong, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

use crate::protocol::ClipKind;
use crate::Engine;

unsafe fn engine_ref<'a>(handle: jlong) -> Option<&'a Engine> {
    if handle == 0 {
        None
    } else {
        Some(&*(handle as *const Engine))
    }
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativeInit<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    data_dir: JString<'local>,
    relay_url: JString<'local>,
) -> jlong {
    let Ok(dir_java) = env.get_string(&data_dir) else {
        return 0;
    };
    let dir_str: String = dir_java.into();

    let relay_str: Option<String> = if relay_url.is_null() {
        None
    } else {
        env.get_string(&relay_url).ok().map(Into::into)
    };

    match Engine::new(dir_str, relay_str.as_deref()) {
        Ok(engine) => Box::into_raw(Box::new(engine)) as jlong,
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativePushLocal<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    kind: jint,
    data: JByteArray<'local>,
) -> jint {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return -1;
    };
    let Some(clip_kind) = ClipKind::from_u8(kind as u8) else {
        return -1;
    };
    let Ok(bytes) = env.convert_byte_array(&data) else {
        return -1;
    };
    match engine.push_local(clip_kind, &bytes) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => -1,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativePopRemote<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> JByteArray<'local> {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return JByteArray::default();
    };
    let Some(clip) = engine.pop_remote() else {
        return JByteArray::default();
    };

    let mime_bytes = clip.mime.as_bytes();
    let mut buf = Vec::with_capacity(1 + 8 + 4 + mime_bytes.len() + 4 + clip.data.len());
    buf.push(clip.kind as u8);
    buf.extend_from_slice(&clip.timestamp_ms.to_be_bytes());
    buf.extend_from_slice(&(mime_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(mime_bytes);
    buf.extend_from_slice(&(clip.data.len() as u32).to_be_bytes());
    buf.extend_from_slice(&clip.data);

    env.byte_array_from_slice(&buf).unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativeWaitEvent<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    timeout_ms: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return JNI_FALSE;
    };
    if engine.wait_event(timeout_ms.max(0) as u64) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativePinItem<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    kind: jint,
    data: JByteArray<'local>,
) -> JByteArray<'local> {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return JByteArray::default();
    };
    let Some(clip_kind) = ClipKind::from_u8(kind as u8) else {
        return JByteArray::default();
    };
    let Ok(bytes) = env.convert_byte_array(&data) else {
        return JByteArray::default();
    };
    match engine.pin_item(clip_kind, &bytes) {
        Ok(pin) => env.byte_array_from_slice(&pin.id).unwrap_or_default(),
        Err(_) => JByteArray::default(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativeUnpinItem<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    id_bytes: JByteArray<'local>,
) -> jboolean {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return JNI_FALSE;
    };
    let Ok(bytes) = env.convert_byte_array(&id_bytes) else {
        return JNI_FALSE;
    };
    if bytes.len() != 16 {
        return JNI_FALSE;
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(&bytes);
    match engine.unpin_item(&id) {
        Ok(true) => JNI_TRUE,
        _ => JNI_FALSE,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativeGetPins<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> JByteArray<'local> {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return JByteArray::default();
    };
    let pins = engine.get_pins();
    let mut buf = Vec::new();
    buf.extend_from_slice(&(pins.len() as u32).to_be_bytes());
    for pin in pins {
        let mime_bytes = pin.mime.as_bytes();
        buf.extend_from_slice(&pin.id);
        buf.push(pin.kind as u8);
        buf.extend_from_slice(&pin.created_at_ms.to_be_bytes());
        buf.extend_from_slice(&pin.updated_at_ms.to_be_bytes());
        buf.extend_from_slice(&(mime_bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(mime_bytes);
        buf.extend_from_slice(&(pin.data.len() as u32).to_be_bytes());
        buf.extend_from_slice(&pin.data);
    }
    env.byte_array_from_slice(&buf).unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativeGetPairingUri<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> JString<'local> {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return JString::default();
    };
    let uri = engine.get_pairing_uri();
    env.new_string(&uri).unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativeJoinPairingUri<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    uri: JString<'local>,
) -> jboolean {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return JNI_FALSE;
    };
    let Ok(uri_java) = env.get_string(&uri) else {
        return JNI_FALSE;
    };
    let uri_str: String = uri_java.into();
    match engine.join_pairing_uri(&uri_str) {
        Ok(_) => JNI_TRUE,
        Err(_) => JNI_FALSE,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativeRecordEcho<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    kind: jint,
    data: JByteArray<'local>,
) {
    let Some(engine) = (unsafe { engine_ref(handle) }) else {
        return;
    };
    let Some(clip_kind) = ClipKind::from_u8(kind as u8) else {
        return;
    };
    if let Ok(bytes) = env.convert_byte_array(&data) {
        engine.record_echo(clip_kind, &bytes);
    }
}

#[no_mangle]
pub extern "system" fn Java_com_clipsync_android_ClipSyncNative_nativeShutdown<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) {
    if handle != 0 {
        let boxed = unsafe { Box::from_raw(handle as *mut Engine) };
        boxed.shutdown();
    }
}
