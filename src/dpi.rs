//! 平台 DPI 能力的运行时探测（Windows 7 兼容层）。
//!
//! 为什么不用 `windows` crate 的 `Win32_UI_HiDpi` 特性：那里的函数都是**静态导入**，
//! 而静态导入的符号解析发生在**进程加载期**，函数不存在时程序连 `main()` 都进不去，
//! 直接报「无法定位程序输入点」。这些函数的最低系统版本分别是：
//!
//! | 函数 | 来源 DLL | 最低系统 |
//! |---|---|---|
//! | `GetDpiForWindow` / `GetDpiForSystem` | `user32.dll` | Windows 10 1607 |
//! | `GetThreadDpiAwarenessContext` / `GetAwarenessFromDpiAwarenessContext` | `user32.dll` | Windows 10 1607 |
//! | `SetProcessDpiAwarenessContext` | `user32.dll` | Windows 10 1703 |
//! | `GetDpiForMonitor` / `SetProcessDpiAwareness` / `GetProcessDpiAwareness` | `shcore.dll` | Windows 8.1 |
//!
//! 所以这里全部改为 `GetProcAddress` 运行时解析，探测失败即回退到更早系统上也有的等价实现。
//!
//! 设计取舍：**能力探测优先于版本判断**。不去读 OS 版本号，只问「这个函数在不在」，
//! 于是同一份代码在 Windows 7 / 8 / 8.1 / 10 / 11 上都自动走当前平台最短的正确路径，
//! 也不会被兼容性层改写的版本号误判。
//!
//! 回退链：
//! - 窗口 DPI：[`dpi_of`] → 系统 DPI
//! - 系统 DPI：[`system_dpi`] → 屏幕 DC 的 `GetDeviceCaps(LOGPIXELSX)`（Windows 2000+）
//! - 显示器 DPI：[`dpi_of_monitor`] → 系统 DPI（Windows 7 没有每显示器 DPI 概念）
//! - 设定感知级别：[`set_process_dpi_awareness`] → 每显示器 → 系统感知

use std::mem::{size_of, transmute_copy};
use std::sync::OnceLock;

use windows::core::PCSTR;
use windows::Win32::Foundation::{HANDLE, HMODULE, HWND};
use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, HMONITOR, LOGPIXELSX};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
use windows::Win32::System::Threading::GetCurrentProcess;

// Windows 7 原生就有的 DPI 感知 API（Vista+，`user32.dll`）也走运行时解析。
//
// 为什么不做静态导入（哪怕写了 `#[link(name = "user32")]`）：MinGW 的 `libuser32.a`
// 里根本没有 `GetProcessDPIAware`，实测链接期报 `undefined reference`。
// 这正是 Rust 标准库在 `api-ms-win-core-synch` 上遇到并注释过的"老 mingw 导入库"问题。
// 全部走 `GetProcAddress` 之后，本模块**不再有任何新增静态导入**。

/// `DPI_AWARENESS`：进程 / 线程的 DPI 感知级别。
pub const DPI_AWARENESS_UNAWARE: i32 = 0;
/// 见 [`DPI_AWARENESS_UNAWARE`]。
pub const DPI_AWARENESS_SYSTEM_AWARE: i32 = 1;
/// 见 [`DPI_AWARENESS_UNAWARE`]。
pub const DPI_AWARENESS_PER_MONITOR_AWARE: i32 = 2;

/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`（Windows 10 1703+，固定伪句柄值）。
const CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
/// `PROCESS_PER_MONITOR_DPI_AWARE`（`PROCESS_DPI_AWARENESS`，Windows 8.1+）。
const PROCESS_PER_MONITOR_DPI_AWARE: i32 = 2;
/// `MDT_EFFECTIVE_DPI`（`MONITOR_DPI_TYPE`）。
const MDT_EFFECTIVE_DPI: i32 = 0;
/// DPI 下限：96（100%）。对外函数一律不低于它。
const MIN_DPI: i32 = 96;

type FnGetDpiForWindow = unsafe extern "system" fn(HWND) -> u32;
type FnGetDpiForSystem = unsafe extern "system" fn() -> u32;
/// `HRESULT GetDpiForMonitor(HMONITOR, MONITOR_DPI_TYPE, UINT*, UINT*)`
type FnGetDpiForMonitor = unsafe extern "system" fn(HMONITOR, i32, *mut u32, *mut u32) -> i32;
/// `BOOL SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT)`
type FnSetAwarenessContext = unsafe extern "system" fn(isize) -> i32;
/// `HRESULT SetProcessDpiAwareness(PROCESS_DPI_AWARENESS)`
type FnSetProcessAwareness = unsafe extern "system" fn(i32) -> i32;
/// `DPI_AWARENESS_CONTEXT GetThreadDpiAwarenessContext()`
type FnGetThreadAwarenessContext = unsafe extern "system" fn() -> isize;
/// `DPI_AWARENESS GetAwarenessFromDpiAwarenessContext(DPI_AWARENESS_CONTEXT)`
type FnGetAwarenessFromContext = unsafe extern "system" fn(isize) -> i32;
/// `HRESULT GetProcessDpiAwareness(HANDLE, PROCESS_DPI_AWARENESS*)`
type FnGetProcessAwareness = unsafe extern "system" fn(HANDLE, *mut i32) -> i32;
/// `BOOL SetProcessDPIAware()`
type FnSetProcessDpiAware = unsafe extern "system" fn() -> i32;
/// `BOOL GetProcessDPIAware()`
type FnGetProcessDpiAware = unsafe extern "system" fn() -> i32;

/// 一次性解析、全程复用的函数表；当前平台没有的项为 `None`。
struct Api {
    get_dpi_for_window: Option<FnGetDpiForWindow>,
    get_dpi_for_system: Option<FnGetDpiForSystem>,
    get_dpi_for_monitor: Option<FnGetDpiForMonitor>,
    set_awareness_context: Option<FnSetAwarenessContext>,
    set_process_awareness: Option<FnSetProcessAwareness>,
    get_thread_awareness_context: Option<FnGetThreadAwarenessContext>,
    get_awareness_from_context: Option<FnGetAwarenessFromContext>,
    get_process_awareness: Option<FnGetProcessAwareness>,
    set_process_dpi_aware: Option<FnSetProcessDpiAware>,
    get_process_dpi_aware: Option<FnGetProcessDpiAware>,
}

/// 取函数表（首次调用时解析）。
fn api() -> &'static Api {
    static API: OnceLock<Api> = OnceLock::new();
    API.get_or_init(Api::resolve)
}

impl Api {
    fn resolve() -> Self {
        // user32 必然已加载（本程序静态导入它）；shcore 在 Windows 8.1 之前不存在，
        // 加载失败即表示"每显示器 DPI"这一族能力全部不可用。
        let user32 = unsafe { GetModuleHandleA(ansi(b"user32.dll\0")) }.unwrap_or_default();
        let shcore = unsafe { LoadLibraryA(ansi(b"shcore.dll\0")) }.unwrap_or_default();
        unsafe {
            Self {
                get_dpi_for_window: sym(user32, b"GetDpiForWindow\0"),
                get_dpi_for_system: sym(user32, b"GetDpiForSystem\0"),
                set_awareness_context: sym(user32, b"SetProcessDpiAwarenessContext\0"),
                get_thread_awareness_context: sym(user32, b"GetThreadDpiAwarenessContext\0"),
                get_awareness_from_context: sym(user32, b"GetAwarenessFromDpiAwarenessContext\0"),
                set_process_dpi_aware: sym(user32, b"SetProcessDPIAware\0"),
                get_process_dpi_aware: sym(user32, b"GetProcessDPIAware\0"),
                get_dpi_for_monitor: sym(shcore, b"GetDpiForMonitor\0"),
                set_process_awareness: sym(shcore, b"SetProcessDpiAwareness\0"),
                get_process_awareness: sym(shcore, b"GetProcessDpiAwareness\0"),
            }
        }
    }
}

/// NUL 结尾的 ASCII 名字字面量 → `PCSTR`。
///
/// 调用方必须写成 `b"Name\0"`：Rust 的字节串字面量**不含**结尾 NUL，
/// 少了它 `GetProcAddress` 会越界读。
fn ansi(name: &'static [u8]) -> PCSTR {
    debug_assert_eq!(name.last(), Some(&0), "符号名必须以 NUL 结尾");
    PCSTR(name.as_ptr())
}

/// 解析一个导出符号。模块句柄无效或符号缺失时返回 `None`。
///
/// SAFETY：`T` 必须是该符号的真实 ABI；调用方只传入与本模块声明一致的函数指针类型。
unsafe fn sym<T: Copy>(module: HMODULE, name: &'static [u8]) -> Option<T> {
    if module.is_invalid() {
        return None;
    }
    // `GetProcAddress` 返回 `Option<FARPROC>`：符号缺失时为空 —— 正是我们要的探测语义。
    let f = unsafe { GetProcAddress(module, ansi(name)) }?;
    debug_assert_eq!(size_of::<T>(), size_of_val(&f));
    Some(unsafe { transmute_copy::<_, T>(&f) })
}

/// 系统 DPI（主显示器的每英寸逻辑像素数），下限 96。
///
/// Windows 10 1607 起走 `GetDpiForSystem`；更早系统用屏幕 DC 的 `LOGPIXELSX`
/// —— 这个值受 DPI 虚拟化影响，而本进程在清单里已声明 DPI 感知，所以拿到的是物理值。
pub fn system_dpi() -> i32 {
    if let Some(f) = api().get_dpi_for_system {
        let d = unsafe { f() } as i32;
        if d >= MIN_DPI {
            return d;
        }
    }
    unsafe {
        // 传 NULL HWND 取整个屏幕的 DC
        let hdc = GetDC(HWND::default());
        if hdc.is_invalid() {
            return MIN_DPI;
        }
        let d = GetDeviceCaps(hdc, LOGPIXELSX);
        let _ = ReleaseDC(HWND::default(), hdc);
        d.max(MIN_DPI)
    }
}

/// 窗口所在显示器的 DPI，下限 96。
///
/// Windows 10 1607 之前没有 `GetDpiForWindow`，回退到系统 DPI
/// —— 那些系统上没有每显示器 DPI，系统 DPI 就是正确答案。
pub fn dpi_of(hwnd: HWND) -> i32 {
    if let Some(f) = api().get_dpi_for_window {
        let d = unsafe { f(hwnd) } as i32;
        if d >= MIN_DPI {
            return d;
        }
    }
    system_dpi()
}

/// 指定显示器的 DPI（`GetDpiForMonitor`，Windows 8.1+），下限 96。
///
/// 不可用时回退到系统 DPI。**不要**回退到硬编码 96：在 125% / 150% 缩放下
/// 那会把贴图的留白与尺寸算错。
pub fn dpi_of_monitor(hmon: HMONITOR) -> i32 {
    if let Some(f) = api().get_dpi_for_monitor {
        let (mut dx, mut dy) = (0u32, 0u32);
        let hr = unsafe { f(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
        if hr >= 0 && dx >= MIN_DPI as u32 {
            return dx as i32;
        }
    }
    system_dpi()
}

/// 设定进程 DPI 感知级别，三级回退（新的优先）：
/// PerMonitorV2（Windows 10 1703+）→ 每显示器感知（Windows 8.1+）→ 系统感知（Vista+）。
///
/// 必须是 `main()` 的第一件事。正常路径由 `app.manifest` 在进程启动前就定死级别，
/// 此时这些调用会因"已被锁定"而失败，属预期结果；这里只是清单缺失/被忽略时的兜底。
pub fn set_process_dpi_awareness() {
    let api = api();
    unsafe {
        if let Some(f) = api.set_awareness_context {
            if f(CONTEXT_PER_MONITOR_AWARE_V2) != 0 {
                return;
            }
        }
        if let Some(f) = api.set_process_awareness {
            if f(PROCESS_PER_MONITOR_DPI_AWARE) >= 0 {
                return;
            }
        }
        // Windows 7 / 8 的兜底：只有系统感知可用
        if let Some(f) = api.set_process_dpi_aware {
            let _ = f();
        }
    }
}

/// 当前实际生效的 DPI 感知级别，取值见 [`DPI_AWARENESS_UNAWARE`] 等常量。
pub fn awareness() -> i32 {
    let api = api();
    unsafe {
        if let (Some(get_ctx), Some(get_awareness)) =
            (api.get_thread_awareness_context, api.get_awareness_from_context)
        {
            return get_awareness(get_ctx());
        }
        if let Some(f) = api.get_process_awareness {
            let mut value = DPI_AWARENESS_UNAWARE;
            if f(GetCurrentProcess(), &mut value) >= 0 {
                return value;
            }
        }
        // Windows 7 / Vista：只有"系统感知 / 非感知"两态
        if let Some(f) = api.get_process_dpi_aware {
            return if f() != 0 {
                DPI_AWARENESS_SYSTEM_AWARE
            } else {
                DPI_AWARENESS_UNAWARE
            };
        }
        // 理论上到不了这里：GetProcessDPIAware 自 Vista 起就在 user32 里
        DPI_AWARENESS_UNAWARE
    }
}

/// 本平台能达到的 DPI 感知上限，用于校验实际级别（日志与冒烟测试的预期值）。
///
/// 清单里的 `dpiAwareness=PerMonitorV2` / `dpiAware=true/PM` 在各平台含义不同：
/// Windows 7 / 8 只按**系统感知**处理，Windows 8.1 / 10 才是每显示器感知。
/// 这里用"能否设置每显示器感知"来判定，而不是读版本号。
pub fn max_awareness() -> i32 {
    let api = api();
    if api.set_awareness_context.is_some() || api.set_process_awareness.is_some() {
        DPI_AWARENESS_PER_MONITOR_AWARE
    } else {
        DPI_AWARENESS_SYSTEM_AWARE
    }
}
