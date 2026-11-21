use std::io::Cursor;
use std::ptr;

use clipsync_core::protocol::ClipKind;
use image::ImageFormat;
use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};

pub enum ClipboardContent {
    Text(Vec<u8>),
    Image(Vec<u8>),
}

pub struct ClipboardFormats {
    exclude_monitor: u32,
    can_history: u32,
    png: u32,
}

impl ClipboardFormats {
    pub fn register() -> Self {
        unsafe {
            Self {
                exclude_monitor: RegisterClipboardFormatW(w!(
                    "ExcludeClipboardContentFromMonitorProcessing"
                )),
                can_history: RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
                png: RegisterClipboardFormatW(w!("PNG")),
            }
        }
    }

    pub fn is_concealed(&self) -> bool {
        unsafe {
            if self.exclude_monitor != 0 && IsClipboardFormatAvailable(self.exclude_monitor).is_ok()
            {
                return true;
            }
            if self.can_history != 0 && IsClipboardFormatAvailable(self.can_history).is_ok() {
                if let Ok(handle) = GetClipboardData(self.can_history) {
                    let hmem = HGLOBAL(handle.0);
                    let ptr = GlobalLock(hmem);
                    if !ptr.is_null() {
                        let allow = if GlobalSize(hmem) >= 4 {
                            *(ptr as *const u32) != 0
                        } else {
                            true
                        };
                        let _ = GlobalUnlock(hmem);
                        if !allow {
                            return true;
                        }
                    }
                }
            }
            false
        }
    }

    pub fn read(&self, hwnd: HWND) -> Option<ClipboardContent> {
        unsafe {
            if OpenClipboard(hwnd).is_err() {
                return None;
            }
            let result = self.read_locked();
            let _ = CloseClipboard();
            result
        }
    }

    unsafe fn read_locked(&self) -> Option<ClipboardContent> {
        if self.is_concealed() {
            return None;
        }

        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
            if let Ok(handle) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
                if let Some(bytes) = copy_global_bytes(HGLOBAL(handle.0)) {
                    if bytes.len() >= 2 {
                        let u16_slice: Vec<u16> = bytes
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .take_while(|&ch| ch != 0)
                            .collect();
                        if let Ok(text) = String::from_utf16(&u16_slice) {
                            if !text.is_empty() {
                                return Some(ClipboardContent::Text(text.into_bytes()));
                            }
                        }
                    }
                }
            }
        }

        if self.png != 0 && IsClipboardFormatAvailable(self.png).is_ok() {
            if let Ok(handle) = GetClipboardData(self.png) {
                if let Some(png_bytes) = copy_global_bytes(HGLOBAL(handle.0)) {
                    if !png_bytes.is_empty() {
                        return Some(ClipboardContent::Image(png_bytes));
                    }
                }
            }
        }

        if IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() {
            if let Ok(handle) = GetClipboardData(CF_DIB.0 as u32) {
                if let Some(dib_bytes) = copy_global_bytes(HGLOBAL(handle.0)) {
                    if let Some(bmp) = wrap_dib_as_bmp(&dib_bytes) {
                        return Some(ClipboardContent::Image(bmp));
                    }
                }
            }
        }

        None
    }

    pub fn write(&self, hwnd: HWND, kind: ClipKind, data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            if OpenClipboard(hwnd).is_err() {
                return None;
            }
            let written = match kind {
                ClipKind::Text => {
                    if self.write_text_locked(data) {
                        Some(data.to_vec())
                    } else {
                        None
                    }
                }
                ClipKind::Image => self.write_image_locked(data),
            };
            let _ = CloseClipboard();
            written
        }
    }

    unsafe fn write_text_locked(&self, utf8: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(utf8) else {
            return false;
        };
        if EmptyClipboard().is_err() {
            return false;
        }
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let byte_slice =
            std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
        alloc_and_set_clipboard(CF_UNICODETEXT.0 as u32, byte_slice)
    }

    unsafe fn write_image_locked(&self, bytes: &[u8]) -> Option<Vec<u8>> {
        let Ok(img) = image::load_from_memory(bytes) else {
            return None;
        };
        if EmptyClipboard().is_err() {
            return None;
        }

        let mut recorded_png: Option<Vec<u8>> = None;

        // Write PNG format for modern apps.
        if self.png != 0 {
            let mut png_buf = Cursor::new(Vec::new());
            if img.write_to(&mut png_buf, ImageFormat::Png).is_ok() {
                let png_vec = png_buf.into_inner();
                if alloc_and_set_clipboard(self.png, &png_vec) {
                    recorded_png = Some(png_vec);
                }
            }
        }

        // Also write CF_DIB (32bpp bottom-up BITMAPINFOHEADER) for Paint / Office / legacy apps.
        let mut wrote_dib = false;
        if let Some(dib) = encode_bottom_up_dib(&img) {
            if alloc_and_set_clipboard(CF_DIB.0 as u32, &dib) {
                wrote_dib = true;
            }
        }

        if recorded_png.is_some() {
            recorded_png
        } else if wrote_dib {
            Some(bytes.to_vec())
        } else {
            None
        }
    }
}

unsafe fn copy_global_bytes(hmem: HGLOBAL) -> Option<Vec<u8>> {
    let size = GlobalSize(hmem);
    if size == 0 {
        return None;
    }
    let ptr = GlobalLock(hmem);
    if ptr.is_null() {
        return None;
    }
    let mut buf = vec![0u8; size];
    ptr::copy_nonoverlapping(ptr as *const u8, buf.as_mut_ptr(), size);
    let _ = GlobalUnlock(hmem);
    Some(buf)
}

unsafe fn alloc_and_set_clipboard(format: u32, data: &[u8]) -> bool {
    let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, data.len()) else {
        return false;
    };
    let ptr = GlobalLock(hmem);
    if ptr.is_null() {
        return false;
    }
    ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
    let _ = GlobalUnlock(hmem);
    SetClipboardData(format, HANDLE(hmem.0)).is_ok()
}

fn wrap_dib_as_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]);
    if header_size < 40 || (header_size as usize) > dib.len() {
        return None;
    }
    let bpp = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    let colors_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]);

    let palette_entries = if colors_used > 0 {
        colors_used
    } else if bpp <= 8 {
        1u32 << bpp
    } else {
        0
    };
    let bitfields_bytes = if header_size == 40 && compression == 3 { 12 } else { 0 };
    let pixel_offset = 14 + header_size + bitfields_bytes + palette_entries * 4;
    let total_size = 14 + dib.len() as u32;

    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&total_size.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&pixel_offset.to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

fn encode_bottom_up_dib(img: &image::DynamicImage) -> Option<Vec<u8>> {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let pixel_bytes = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    let mut dib = Vec::with_capacity(40 + pixel_bytes);

    // BITMAPINFOHEADER (40 bytes)
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(w as i32).to_le_bytes());
    dib.extend_from_slice(&(h as i32).to_le_bytes()); // positive = bottom-up
    dib.extend_from_slice(&1u16.to_le_bytes()); // planes
    dib.extend_from_slice(&32u16.to_le_bytes()); // bpp (BGRA)
    dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    dib.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());

    let raw = rgba.as_raw();
    let row_bytes = (w as usize) * 4;
    for y in (0..h as usize).rev() {
        let row = &raw[y * row_bytes..(y + 1) * row_bytes];
        for px in row.chunks_exact(4) {
            dib.push(px[2]); // B
            dib.push(px[1]); // G
            dib.push(px[0]); // R
            dib.push(px[3]); // A
        }
    }
    Some(dib)
}
