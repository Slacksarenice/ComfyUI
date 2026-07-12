//! WinUI 3 desktop shell for ComfyUI, in pure Rust via windows-rs.
//!
//! WinUI 3 content (Microsoft.UI.Xaml from the Windows App SDK) is hosted in
//! a Win32 window through `DesktopWindowXamlSource` (WinUI 3 islands). This
//! avoids WinRT class composition (subclassing `Application`), which
//! windows-rs does not support yet, while keeping the whole UI stack native
//! WinUI 3. When packaged as MSIX the Windows App Runtime comes from the
//! framework-package dependency; unpackaged runs bootstrap it dynamically.

#[allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
mod bindings;

use std::cell::RefCell;

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Graphics::RectInt32;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_SINGLETHREADED};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, PostQuitMessage,
    RegisterClassW, ShowWindow, TranslateMessage, CW_USEDEFAULT, MSG, SW_SHOW, WINDOW_EX_STYLE,
    WM_DESTROY, WM_SIZE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use bindings::Microsoft::UI::Content::DesktopChildSiteBridge;
use bindings::Microsoft::UI::Dispatching::DispatcherQueueController;
use bindings::Microsoft::UI::WindowId;
use bindings::Microsoft::UI::Xaml::Controls::{StackPanel, TextBlock};
use bindings::Microsoft::UI::Xaml::Hosting::{DesktopWindowXamlSource, WindowsXamlManager};

thread_local! {
    static SITE_BRIDGE: RefCell<Option<DesktopChildSiteBridge>> = const { RefCell::new(None) };
}

/// MddBootstrapInitialize2 from Microsoft.WindowsAppRuntime.Bootstrap.dll.
/// PACKAGE_VERSION is a union over a u64, passed by value.
type MddBootstrapInitialize2 =
    unsafe extern "system" fn(u32, PCWSTR, u64, u32) -> windows::core::HRESULT;

const WINDOWSAPPSDK_RELEASE_MAJORMINOR: u32 = 0x0002_0002; // 2.2
const MDD_ON_NO_MATCH_SHOW_UI: u32 = 0x08;
const MDD_ON_PACKAGE_IDENTITY_NOOP: u32 = 0x10;

/// Load the Windows App Runtime when running without package identity.
/// A no-op inside an MSIX package (the framework dependency provides it).
fn bootstrap_windows_app_runtime() -> Result<()> {
    unsafe {
        let module = LoadLibraryW(w!("Microsoft.WindowsAppRuntime.Bootstrap.dll"))?;
        let proc = GetProcAddress(module, windows::core::s!("MddBootstrapInitialize2"))
            .expect("MddBootstrapInitialize2 not found in bootstrap DLL");
        let init: MddBootstrapInitialize2 = std::mem::transmute(proc);
        init(
            WINDOWSAPPSDK_RELEASE_MAJORMINOR,
            w!(""),
            0,
            MDD_ON_NO_MATCH_SHOW_UI | MDD_ON_PACKAGE_IDENTITY_NOOP,
        )
        .ok()
    }
}

/// Build the placeholder WinUI 3 content. As migration proceeds this becomes
/// the real workflow UI; for now it proves the XAML stack end to end and
/// shows a scheduling result computed by the comfy-graph crate.
fn build_content() -> Result<StackPanel> {
    let panel = StackPanel::new()?;

    let title = TextBlock::new()?;
    title.SetText(&HSTRING::from("ComfyUI native shell (WinUI 3 + Rust)"))?;
    panel.Children()?.Append(&title)?;

    let detail = TextBlock::new()?;
    detail.SetText(&HSTRING::from(format!(
        "comfy-graph execution order for demo prompt: {}",
        demo_execution_order().join(" -> ")
    )))?;
    panel.Children()?.Append(&detail)?;

    Ok(panel)
}

/// Exercise the ported execution-graph code against a small prompt.
fn demo_execution_order() -> Vec<String> {
    use comfy_graph::{DynamicPrompt, NoopResolver, TopologicalSort};
    let prompt: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
        r#"{
            "save":   {"class_type": "SaveImage",  "inputs": {"images": ["decode", 0]}},
            "decode": {"class_type": "VAEDecode",  "inputs": {"samples": ["sample", 0]}},
            "sample": {"class_type": "KSampler",   "inputs": {"model": ["load", 0]}},
            "load":   {"class_type": "Checkpoint", "inputs": {}}
        }"#,
    )
    .unwrap();
    let prompt = DynamicPrompt::new(prompt);
    let resolver = NoopResolver;
    let mut sort = TopologicalSort::new(&prompt, &resolver);
    sort.add_node("save", false, None).unwrap();
    sort.execution_order().unwrap()
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            let width = (lparam.0 & 0xFFFF) as i32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as i32;
            SITE_BRIDGE.with_borrow(|bridge| {
                if let Some(bridge) = bridge {
                    let _ = bridge.MoveAndResize(RectInt32 {
                        X: 0,
                        Y: 0,
                        Width: width,
                        Height: height,
                    });
                }
            });
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn run() -> Result<()> {
    bootstrap_windows_app_runtime()?;

    unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };

    // XAML needs a DispatcherQueue on this thread; the Win32 message loop
    // below pumps it.
    let _dispatcher = DispatcherQueueController::CreateOnCurrentThread()?;
    let _xaml_manager = WindowsXamlManager::InitializeForCurrentThread()?;

    let instance = unsafe { GetModuleHandleW(None)? };
    let class_name = w!("ComfyShellWindow");
    let wc = WNDCLASSW {
        hInstance: instance.into(),
        lpszClassName: class_name,
        lpfnWndProc: Some(wndproc),
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&wc) };
    debug_assert!(atom != 0);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("ComfyUI"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            700,
            None,
            None,
            Some(instance.into()),
            None,
        )?
    };

    // Attach a WinUI 3 island to the window and hand it the XAML tree.
    let xaml_source = DesktopWindowXamlSource::new()?;
    let window_id = WindowId {
        Value: hwnd.0 as usize as u64,
    };
    xaml_source.Initialize(window_id)?;
    xaml_source.SetContent(&build_content()?)?;

    let bridge = xaml_source.SiteBridge()?;
    let mut client = windows::Win32::Foundation::RECT::default();
    unsafe { GetClientRect(hwnd, &mut client)? };
    bridge.MoveAndResize(RectInt32 {
        X: 0,
        Y: 0,
        Width: client.right - client.left,
        Height: client.bottom - client.top,
    })?;
    bridge.Show()?;
    SITE_BRIDGE.with_borrow_mut(|slot| *slot = Some(bridge));

    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    SITE_BRIDGE.with_borrow_mut(|slot| *slot = None);
    xaml_source.Close()?;
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("comfy-shell failed: {err:?}");
        std::process::exit(1);
    }
}
