//! Тонкая обёртка над Win32: положение окон, масштаб, курсор, перетаскивание. Все функции безопасны для вызова из любого потока.
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM, COLORREF};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, ClientToScreen, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;

pub type Rect = (i32, i32, i32, i32); // x, y, ширина, высота (физические пиксели)

pub fn h(hwnd: isize) -> HWND {
    HWND(hwnd as *mut core::ffi::c_void)
}

pub fn rect(hwnd: isize) -> Rect {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h(hwnd), &mut r);
    }
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}

pub fn scale(hwnd: isize) -> f64 {
    let d = unsafe { GetDpiForWindow(h(hwnd)) };
    if d == 0 {
        system_scale()
    } else {
        (d as f64 / 96.0).max(1.0)
    }
}

pub fn system_scale() -> f64 {
    ((unsafe { GetDpiForSystem() }) as f64 / 96.0).max(1.0)
}

/// Двигает/меняет размер окна, не дожидаясь потока окна.
pub fn place(hwnd: isize, x: Option<i32>, y: Option<i32>, w: Option<i32>, hh: Option<i32>) {
    let mut flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
    if x.is_none() || y.is_none() {
        flags |= SWP_NOMOVE;
    }
    if w.is_none() || hh.is_none() {
        flags |= SWP_NOSIZE;
    }
    unsafe {
        let _ = SetWindowPos(h(hwnd), None, x.unwrap_or(0), y.unwrap_or(0), w.unwrap_or(0), hh.unwrap_or(0), flags);
    }
}

pub fn place_rect(hwnd: isize, r: Rect) {
    place(hwnd, Some(r.0), Some(r.1), Some(r.2), Some(r.3));
}

/// Прямоугольник монитора окна: (x, y, ширина, высота).
pub fn work_area(hwnd: isize, full: bool) -> Rect {
    unsafe {
        let mon = MonitorFromWindow(h(hwnd), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: core::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let r = if full { mi.rcMonitor } else { mi.rcWork };
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
    }
}

/// Все мониторы вместе (физические пиксели).
pub fn virtual_screen() -> Rect {
    unsafe {
        (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
    }
}

pub fn client_rect(hwnd: isize) -> Rect {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(h(hwnd), &mut rc);
        let mut pt = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(h(hwnd), &mut pt);
        (pt.x, pt.y, rc.right - rc.left, rc.bottom - rc.top)
    }
}

/// Двигает окно так, чтобы его содержимое совпало с target.
pub fn align_client(hwnd: isize, target: Rect) {
    let o = rect(hwnd);
    let c = client_rect(hwnd);
    place(hwnd, Some(target.0 - (c.0 - o.0)), Some(target.1 - (c.1 - o.1)), Some(target.2 + (o.2 - c.2)), Some(target.3 + (o.3 - c.3)));
}

pub fn topmost(hwnd: isize, on: bool) {
    unsafe {
        let _ = SetWindowPos(h(hwnd), Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}

pub fn minimize(hwnd: isize) {
    unsafe {
        let _ = ShowWindowAsync(h(hwnd), SW_MINIMIZE);
    }
}

pub fn restore(hwnd: isize) {
    unsafe {
        let _ = ShowWindowAsync(h(hwnd), SW_RESTORE);
    }
}

pub fn close(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(Some(h(hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn is_alive(hwnd: isize) -> bool {
    unsafe { IsWindow(Some(h(hwnd))).as_bool() }
}
pub fn is_minimized(hwnd: isize) -> bool {
    unsafe { IsIconic(h(hwnd)).as_bool() }
}
pub fn is_visible(hwnd: isize) -> bool {
    unsafe { IsWindowVisible(h(hwnd)).as_bool() }
}

pub fn set_opacity(hwnd: isize, value: f64) {
    let v = value.clamp(0.3, 1.0);
    unsafe {
        let ex = GetWindowLongPtrW(h(hwnd), GWL_EXSTYLE);
        SetWindowLongPtrW(h(hwnd), GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
        let _ = SetLayeredWindowAttributes(h(hwnd), COLORREF(0), (255.0 * v) as u8, LWA_ALPHA);
    }
}

/// Прячет заголовок окна Windows, оставляя рамку для изменения размера.
pub fn hide_caption(hwnd: isize) {
    unsafe {
        let style = GetWindowLongPtrW(h(hwnd), GWL_STYLE);
        let style = (style & !(WS_CAPTION.0 as isize)) | WS_THICKFRAME.0 as isize;
        SetWindowLongPtrW(h(hwnd), GWL_STYLE, style);
        let _ = SetWindowPos(h(hwnd), None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_ASYNCWINDOWPOS);
    }
}

pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// Окно верхнего уровня под курсором.
pub fn root_at_cursor() -> isize {
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let w = WindowFromPoint(p);
        GetAncestor(w, GA_ROOT).0 as isize
    }
}

pub fn left_button_down() -> bool {
    unsafe { (GetAsyncKeyState(0x01) as u16 & 0x8000) != 0 }
}

/// Перетаскивание окна: пока зажата левая кнопка мыши, окно идёт за курсором.
pub fn drag(hwnd: isize, mut on_move: impl FnMut()) {
    let (cx, cy) = cursor();
    let (x0, y0, _, _) = rect(hwnd);
    let (ox, oy) = (cx - x0, cy - y0);
    let t0 = std::time::Instant::now();
    while left_button_down() && t0.elapsed().as_secs() < 60 {
        let (px, py) = cursor();
        place(hwnd, Some(px - ox), Some(py - oy), None, None);
        on_move();
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

/// Панель: окно-инструмент без активации.
pub fn panel_setup(hwnd: isize) {
    unsafe {
        let ex = GetWindowLongPtrW(h(hwnd), GWL_EXSTYLE);
        SetWindowLongPtrW(h(hwnd), GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as isize | WS_EX_NOACTIVATE.0 as isize);
        // чистое окно без рамок: содержимое занимает весь прямоугольник окна
        SetWindowLongPtrW(h(hwnd), GWL_STYLE, (WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN).0 as isize);
        let _ = SetWindowPos(h(hwnd), None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        let _ = ShowWindowAsync(h(hwnd), SW_HIDE);
    }
}

pub fn panel_visible(hwnd: isize, on: bool) {
    unsafe {
        let _ = ShowWindowAsync(h(hwnd), if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

/// Панель выше окна плеера: закреплённое - «поверх всего»; при откреплении сначала снимаем «поверх всего».
pub fn panel_z(hwnd: isize, pinned: bool, reset: bool) {
    unsafe {
        if reset {
            let _ = SetWindowPos(h(hwnd), Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
        }
        let _ = SetWindowPos(h(hwnd), Some(if pinned { HWND_TOPMOST } else { HWND_TOP }), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}

/// Текст из буфера обмена.
pub fn clipboard_text() -> String {
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows::Win32::Foundation::HGLOBAL;
    unsafe {
        if OpenClipboard(None).is_err() {
            return String::new();
        }
        let mut out = String::new();
        if let Ok(hd) = GetClipboardData(13) {
            let hg = HGLOBAL(hd.0);
            let p = GlobalLock(hg) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 && n < 1_000_000 {
                    n += 1;
                }
                out = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                let _ = GlobalUnlock(hg);
            }
        }
        let _ = CloseClipboard();
        out
    }
}

/// Шифрование/расшифровка DPAPI (доступно только вашей учётной записи Windows).
pub fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    unsafe {
        let inp = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        let ok = if protect {
            CryptProtectData(&inp, windows::core::PCWSTR::null(), None, None, None, 0, &mut out).is_ok()
        } else {
            CryptUnprotectData(&inp, None, None, None, None, 0, &mut out).is_ok()
        };
        if !ok || out.pbData.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut core::ffi::c_void)));
        Some(v)
    }
}
