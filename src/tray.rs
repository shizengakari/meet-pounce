use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use tokio::sync::mpsc;

// Win32 定数
const WM_USER: u32 = 0x0400;
const WM_TRAYICON: u32 = WM_USER + 1;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_COMMAND: u32 = 0x0111;
const WM_DESTROY: u32 = 0x0002;

const NIM_ADD: u32 = 0x00000000;
const NIM_MODIFY: u32 = 0x00000001;
const NIM_DELETE: u32 = 0x00000002;

const NIF_MESSAGE: u32 = 0x00000001;
const NIF_ICON: u32 = 0x00000002;
const NIF_TIP: u32 = 0x00000004;

const MF_STRING: u32 = 0x00000000;
const MF_SEPARATOR: u32 = 0x00000800;
const TPM_RIGHTBUTTON: u32 = 0x0002;

const IDI_APPLICATION: usize = 32512;
const IMAGE_ICON: u32 = 1;
const LR_DEFAULTCOLOR: u32 = 0x0000;
const LR_SHARED: u32 = 0x8000;
const SM_CXSMICON: i32 = 49;
const SM_CYSMICON: i32 = 50;

const IDM_SETTINGS: usize = 1001;
const IDM_SYNC: usize = 1002;
const IDM_TEST: usize = 1003;
const IDM_QUIT: usize = 1004;

#[repr(C)]
#[allow(non_snake_case)]
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
#[allow(non_snake_case)]
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
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
#[allow(non_snake_case)]
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
    fn LoadImageW(
        hInst: isize,
        name: usize,
        type_: u32,
        cx: i32,
        cy: i32,
        fuLoad: u32,
    ) -> isize;
    fn GetSystemMetrics(nIndex: i32) -> i32;
    fn PostMessageW(hWnd: isize, Msg: u32, wParam: usize, lParam: isize) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn Shell_NotifyIconW(dwMessage: u32, lpData: *const NOTIFYICONDATAW) -> i32;
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

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_TRAYICON => {
            let event = (lparam & 0xFFFF) as u32;
            if event == WM_RBUTTONUP {
                let hmenu = CreatePopupMenu();
                let s_settings = to_wide("設定");
                let s_sync = to_wide("今すぐ同期");
                let s_test = to_wide("通知テスト");
                let s_quit = to_wide("終了");

                AppendMenuW(hmenu, MF_STRING, IDM_SETTINGS, s_settings.as_ptr());
                AppendMenuW(hmenu, MF_STRING, IDM_SYNC, s_sync.as_ptr());
                AppendMenuW(hmenu, MF_STRING, IDM_TEST, s_test.as_ptr());
                AppendMenuW(hmenu, MF_SEPARATOR, 0, null());
                AppendMenuW(hmenu, MF_STRING, IDM_QUIT, s_quit.as_ptr());

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
            let id = wparam & 0xFFFF;
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

        let cx = GetSystemMetrics(SM_CXSMICON);
        let cy = GetSystemMetrics(SM_CYSMICON);
        let mut hicon = LoadImageW(
            hinstance,
            1,
            IMAGE_ICON,
            cx,
            cy,
            LR_DEFAULTCOLOR | LR_SHARED,
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

        let tip = to_wide("Meet Pounce - 常駐中");
        for (i, &c) in tip.iter().take(127).enumerate() {
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

/// トレイアイコンのツールチップ（次の予定）を更新する
pub fn update_tray_tooltip(tip_text: &str) {
    let hwnd = TRAY_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }

    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_TIP;

        let wide = to_wide(tip_text);
        for (i, &c) in wide.iter().take(127).enumerate() {
            nid.szTip[i] = c;
        }

        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}
