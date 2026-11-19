use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::Cell;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, NTSTATUS, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WINDOWPOS, WM_DPICHANGED,
    WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_WINDOWPOSCHANGING,
};

const SUBCLASS_ID: usize = 0x4E54_5844;

thread_local! {
    static IN_SIZE_MOVE: Cell<bool> = const { Cell::new(false) };
    static PENDING_DPI_RECT: Cell<Option<RECT>> = const { Cell::new(None) };
}

struct PendingRectGuard(Option<RECT>);

impl PendingRectGuard {
    fn set(rect: Option<RECT>) -> Self {
        let prev = PENDING_DPI_RECT.with(|c| c.replace(rect));
        Self(prev)
    }
}

impl Drop for PendingRectGuard {
    fn drop(&mut self) {
        let prev = self.0;
        PENDING_DPI_RECT.with(|c| c.set(prev));
    }
}

fn is_windows_11_or_later() -> bool {
    static IS_WIN11: OnceLock<bool> = OnceLock::new();
    *IS_WIN11.get_or_init(|| {
        type RtlGetVersionFn = unsafe extern "system" fn(*mut OSVERSIONINFOW) -> NTSTATUS;

        let ntdll = unsafe { GetModuleHandleA(c"ntdll.dll".as_ptr().cast()) };
        if ntdll.is_null() {
            return false;
        }
        let Some(proc) = (unsafe { GetProcAddress(ntdll, c"RtlGetVersion".as_ptr().cast()) })
        else {
            return false;
        };
        #[allow(clippy::missing_transmute_annotations)]
        let rtl_get_version: RtlGetVersionFn = unsafe { std::mem::transmute(proc) };

        let mut vi = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            dwMajorVersion: 0,
            dwMinorVersion: 0,
            dwBuildNumber: 0,
            dwPlatformId: 0,
            szCSDVersion: [0; 128],
        };

        let status = unsafe { rtl_get_version(&mut vi) };
        status >= 0 && (vi.dwMajorVersion > 10 || (vi.dwMajorVersion == 10 && vi.dwBuildNumber >= 22000))
    })
}

/// Install a Win32 subclass hook that substitutes Windows' `WM_DPICHANGED`
/// `suggested_rect` when `winit 0.30.13` attempts its buggy boundary-nudge loop.
pub fn install(cc: &eframe::CreationContext<'_>) {
    let Ok(handle) = cc.window_handle() else {
        tracing::warn!("Impossible de récupérer le handle fenêtre Win32 pour le correctif DPI");
        return;
    };

    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };

    let hwnd = win32.hwnd.get() as HWND;
    let ok = unsafe { SetWindowSubclass(hwnd, Some(dpi_subclass_proc), SUBCLASS_ID, 0) };
    if ok == 0 {
        tracing::warn!("Échec de l'installation du subclass Win32 pour WM_DPICHANGED");
    } else {
        tracing::debug!("Correctif Win32 WM_DPICHANGED multi-écrans installé");
    }
}

unsafe extern "system" fn dpi_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid_subclass: usize,
    _dw_ref_data: usize,
) -> LRESULT {
    match msg {
        WM_ENTERSIZEMOVE => {
            IN_SIZE_MOVE.with(|c| c.set(true));
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_EXITSIZEMOVE => {
            IN_SIZE_MOVE.with(|c| c.set(false));
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_DPICHANGED => {
            let should_fix = lparam != 0
                && (is_windows_11_or_later() || IN_SIZE_MOVE.with(|c| c.get()));
            if should_fix {
                let suggested = unsafe { *(lparam as *const RECT) };
                if suggested.right > suggested.left && suggested.bottom > suggested.top {
                    let _guard = PendingRectGuard::set(Some(suggested));
                    return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
                }
            }
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_WINDOWPOSCHANGING => {
            if lparam != 0 {
                if let Some(suggested) = PENDING_DPI_RECT.with(|c| c.get()) {
                    let wp = unsafe { &mut *(lparam as *mut WINDOWPOS) };
                    let is_dpi_reposition = (wp.flags & SWP_NOZORDER) != 0
                        && (wp.flags & SWP_NOACTIVATE) != 0
                        && (wp.flags & (SWP_NOMOVE | SWP_NOSIZE)) == 0;
                    if is_dpi_reposition {
                        PENDING_DPI_RECT.with(|c| c.set(None));
                        wp.x = suggested.left;
                        wp.y = suggested.top;
                        wp.cx = suggested.right - suggested.left;
                        wp.cy = suggested.bottom - suggested.top;
                    }
                }
            }
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_NCDESTROY => {
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(dpi_subclass_proc), SUBCLASS_ID) };
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}
