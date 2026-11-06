use crate::config::{is_startup_enabled, set_startup, to_wide};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use tokio::sync::mpsc;

const WM_USER: u32 = 0x0400;
const WM_TRAYICON: u32 = WM_USER + 1;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_COMMAND: u32 = 0x0111;
const WM_DESTROY: u32 = 0x0002;

const NIM_ADD: u32 = 0x0000_0000;
const NIM_MODIFY: u32 = 0x0000_0001;
const NIM_DELETE: u32 = 0x0000_0002;

const NIF_MESSAGE: u32 = 0x0000_0001;
const NIF_ICON: u32 = 0x0000_0002;
const NIF_TIP: u32 = 0x0000_0004;

const MF_STRING: u32 = 0x0000_0000;
const MF_CHECKED: u32 = 0x0000_0008;
const MF_SEPARATOR: u32 = 0x0000_0800;
const TPM_RIGHTBUTTON: u32 = 0x0002;

const IDI_APPLICATION: usize = 32512;
const IMAGE_ICON: u32 = 1;
const LR_SHARED: u32 = 0x8000;
const SM_CXSMICON: i32 = 49;
const SM_CYSMICON: i32 = 50;

const IDM_SETTINGS: usize = 1001;
const IDM_SYNC: usize = 1002;
const IDM_TEST: usize = 1003;
const IDM_STARTUP: usize = 1004;
const IDM_QUIT: usize = 1005;

#[repr(C)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
struct NOTIFYICONDATAW {
    cbSize: u32,
    hWnd: isize,
    uID: u32,
    uFlags: u32,
    uCallbackMessage: u32,
    hIcon: isize,
    szTip: [u16; 128],
    dwState: u32,
    dwStateMask: u32,
    szInfo: [u16; 256],
    uTimeoutOrVersion: u32,
    szInfoTitle: [u16; 64],
    dwInfoFlags: u32,
    guidItem: [u8; 16],
    hBalloonIcon: isize,
}

#[repr(C)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
struct WNDCLASSW {
    style: u32,
    lpfnWndProc: unsafe extern "system" fn(isize, u32, usize, isize) -> isize,
    cbClsExtra: i32,
    cbWndExtra: i32,
    hInstance: isize,
    hIcon: isize,
    hCursor: isize,
    hbrBackground: isize,
    lpszMenuName: *const u16,
    lpszClassName: *const u16,
}

#[repr(C)]
#[allow(clippy::upper_case_acronyms)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
struct MSG {
    hwnd: isize,
    message: u32,
    wParam: usize,
    lParam: isize,
    time: u32,
    pt: POINT,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lpModuleName: *const u16) -> isize;
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(lpWndClass: *const WNDCLASSW) -> u16;
    fn CreateWindowExW(
        dwExStyle: u32,
        lpClassName: *const u16,
        lpWindowName: *const u16,
        dwStyle: u32,
        X: i32,
        Y: i32,
        nWidth: i32,
        nHeight: i32,
        hWndParent: isize,
        hMenu: isize,
        hInstance: isize,
        lpParam: *mut std::ffi::c_void,
    ) -> isize;
    fn DefWindowProcW(hWnd: isize, Msg: u32, wParam: usize, lParam: isize) -> isize;
    fn GetMessageW(lpMsg: *mut MSG, hWnd: isize, wMsgFilterMin: u32, wMsgFilterMax: u32) -> i32;
    fn TranslateMessage(lpMsg: *const MSG) -> i32;
    fn DispatchMessageW(lpMsg: *const MSG) -> isize;
    fn PostQuitMessage(nExitCode: i32);
    fn CreatePopupMenu() -> isize;
    fn AppendMenuW(hMenu: isize, uFlags: u32, uIDNewItem: usize, lpNewItem: *const u16) -> i32;
    fn TrackPopupMenu(
        hMenu: isize,
        uFlags: u32,
        x: i32,
        y: i32,
        nReserved: i32,
        hWnd: isize,
        prcRect: *const std::ffi::c_void,
    ) -> i32;
    fn DestroyMenu(hMenu: isize) -> i32;
    fn GetCursorPos(lpPoint: *mut POINT) -> i32;
    fn SetForegroundWindow(hWnd: isize) -> i32;
    fn LoadIconW(hInstance: isize, lpIconName: usize) -> isize;
    fn LoadImageW(hInst: isize, name: usize, type_: u32, cx: i32, cy: i32, fuLoad: u32) -> isize;
    fn GetSystemMetrics(nIndex: i32) -> i32;
    fn PostMessageW(hWnd: isize, Msg: u32, wParam: usize, lParam: isize) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn Shell_NotifyIconW(dwMessage: u32, lpData: *const NOTIFYICONDATAW) -> i32;
    fn ShellExecuteW(
        hwnd: isize,
        lpOperation: *const u16,
        lpFile: *const u16,
        lpParameters: *const u16,
        lpDirectory: *const u16,
        nShowCmd: i32,
    ) -> isize;
}

/// 既定のブラウザでURLを開く
pub fn open_browser(url: &str) {
    let op = to_wide("open");
    let file = to_wide(url);
    unsafe {
        ShellExecuteW(0, op.as_ptr(), file.as_ptr(), null(), null(), 1);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    OpenSettings,
    SyncNow,
    TestToast,
    Quit,
}

static TRAY_CMD_TX: std::sync::OnceLock<mpsc::UnboundedSender<TrayCommand>> = std::sync::OnceLock::new();
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_TRAYICON => {
            let event = (lparam & 0xFFFF) as u32;
            if event == WM_RBUTTONUP {
                let hmenu = CreatePopupMenu();
                let startup_flags = if is_startup_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };

                for (flags, id, label) in [
                    (MF_STRING, IDM_SETTINGS, "設定"),
                    (MF_STRING, IDM_SYNC, "今すぐ同期"),
                    (MF_STRING, IDM_TEST, "通知テスト"),
                    (MF_SEPARATOR, 0, ""),
                    (startup_flags, IDM_STARTUP, "スタートアップ登録"),
                    (MF_SEPARATOR, 0, ""),
                    (MF_STRING, IDM_QUIT, "終了"),
                ] {
                    if flags == MF_SEPARATOR {
                        AppendMenuW(hmenu, MF_SEPARATOR, 0, null());
                    } else {
                        let w = to_wide(label);
                        AppendMenuW(hmenu, flags, id, w.as_ptr());
                    }
                }

                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                SetForegroundWindow(hwnd);
                TrackPopupMenu(hmenu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null());
                PostMessageW(hwnd, 0, 0, 0);
                DestroyMenu(hmenu);
            } else if event == WM_LBUTTONDBLCLK {
                if let Some(tx) = TRAY_CMD_TX.get() {
                    let _ = tx.send(TrayCommand::OpenSettings);
                }
            }
            0
        }
        WM_COMMAND => {
            match wparam & 0xFFFF {
                IDM_STARTUP => {
                    let _ = set_startup(!is_startup_enabled());
                }
                id => {
                    if let Some(tx) = TRAY_CMD_TX.get() {
                        match id {
                            IDM_SETTINGS => { let _ = tx.send(TrayCommand::OpenSettings); }
                            IDM_SYNC => { let _ = tx.send(TrayCommand::SyncNow); }
                            IDM_TEST => { let _ = tx.send(TrayCommand::TestToast); }
                            IDM_QUIT => {
                                let _ = tx.send(TrayCommand::Quit);
                                PostQuitMessage(0);
                            }
                            _ => {}
                        }
                    }
                }
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// タスクトレイアイコンを初期化してメッセージループを別スレッドで開始する
pub fn init_tray(tx: mpsc::UnboundedSender<TrayCommand>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = TRAY_CMD_TX.set(tx);

    std::thread::spawn(|| unsafe {
        let hinstance = GetModuleHandleW(null());
        let class_name = to_wide("MeetPounceTrayClass");

        let mut hicon = LoadImageW(
            hinstance,
            1,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_SHARED,
        );
        if hicon == 0 {
            hicon = LoadIconW(hinstance, 1);
        }
        if hicon == 0 {
            hicon = LoadIconW(0, IDI_APPLICATION);
        }

        let wnd_class = WNDCLASSW {
            style: 0,
            lpfnWndProc: wnd_proc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: hicon,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
        };

        RegisterClassW(&wnd_class);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            hinstance,
            null_mut(),
        );
        if hwnd == 0 {
            return;
        }

        TRAY_HWND.store(hwnd, Ordering::SeqCst);

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = hicon;

        for (i, &c) in to_wide("Meet Pounce - 常駐中").iter().take(127).enumerate() {
            nid.szTip[i] = c;
        }

        Shell_NotifyIconW(NIM_ADD, &nid);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        Shell_NotifyIconW(NIM_DELETE, &nid);
    });

    Ok(())
}

static LAST_TIP_HASH: AtomicU64 = AtomicU64::new(0);

/// トレイアイコンのツールチップを更新する
pub fn update_tray_tooltip(tip_text: &str) {
    let hwnd = TRAY_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }

    let mut hasher = DefaultHasher::new();
    tip_text.hash(&mut hasher);
    let hash = hasher.finish();
    if LAST_TIP_HASH.swap(hash, Ordering::SeqCst) == hash {
        return;
    }

    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_TIP;

        for (i, &c) in to_wide(tip_text).iter().take(127).enumerate() {
            nid.szTip[i] = c;
        }

        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}
