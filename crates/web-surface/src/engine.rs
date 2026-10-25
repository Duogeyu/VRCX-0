//! Windows WebView2 engine for the offscreen web surface.
//!
//! One thread owns a hidden host window, an STA COM apartment, the WebView2
//! controller and a Win32 message loop. Captured frames are published through
//! [`Shared`]; input events arrive over an mpsc channel and are re-posted into
//! the hosted browser as window messages.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CapturePreviewCompletedHandler, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
};
use windows::core::{Interface, BOOL};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, IStream, COINIT_APARTMENTTHREADED, STREAM_SEEK_SET,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::{Shared, WebInputEvent, WebSurfaceConfig, WebSurfaceStatus};

const TIMER_TICK: usize = 1;
const TICK_MS: u32 = 16;

thread_local! {
    static ENGINE: RefCell<Option<Rc<RefCell<EngineState>>>> = const { RefCell::new(None) };
}

struct EngineState {
    cfg: WebSurfaceConfig,
    shared: Arc<Shared>,
    controller: Option<ICoreWebView2Controller>,
    view: Option<ICoreWebView2>,
    host: HWND,
    child: HWND,
    capturing: Arc<std::sync::atomic::AtomicBool>,
    last_capture: Instant,
    ready: bool,
}

fn null_hwnd() -> HWND {
    HWND(std::ptr::null_mut())
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            if wparam.0 == TIMER_TICK {
                ENGINE.with(|engine| {
                    if let Some(state) = engine.borrow().as_ref() {
                        let mut state = state.borrow_mut();
                        state.poll_capture();
                    }
                });
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = unsafe { KillTimer(Some(hwnd), TIMER_TICK) };
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

impl EngineState {
    fn poll_capture(&mut self) {
        if !self.ready || self.capturing.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        if self.last_capture.elapsed() < Duration::from_millis(self.cfg.refresh_ms.max(33)) {
            return;
        }
        self.last_capture = Instant::now();
        self.start_capture();
    }

    fn start_capture(&mut self) {
        let Some(view) = self.view.clone() else {
            return;
        };
        let Some(stream) = (unsafe { SHCreateMemStream(None) }) else {
            tracing::warn!("web overlay: cannot allocate capture stream");
            return;
        };
        self.capturing
            .store(true, std::sync::atomic::Ordering::Release);
        let capturing = Arc::clone(&self.capturing);
        let shared = Arc::clone(&self.shared);
        let stream_for_cb = stream.clone();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |hr| {
            if hr.is_ok() {
                match read_stream(&stream_for_cb).and_then(|bytes| decode_frame(&bytes)) {
                    Some((width, height, rgba)) => shared.publish_frame(width, height, rgba),
                    None => tracing::warn!("web overlay: failed to decode captured frame"),
                }
            } else {
                tracing::warn!(?hr, "web overlay: CapturePreview failed");
            }
            capturing.store(false, std::sync::atomic::Ordering::Release);
            Ok(())
        }));
        let result = unsafe {
            view.CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
                &stream,
                &handler,
            )
        };
        if result.is_err() {
            self.capturing
                .store(false, std::sync::atomic::Ordering::Release);
            tracing::warn!(
                error = ?result.err(),
                "web overlay: CapturePreview call failed"
            );
        }
    }

    fn apply_input(&mut self, event: WebInputEvent) -> bool {
        match event {
            WebInputEvent::Shutdown => return false,
            WebInputEvent::Navigate(url) => {
                if let Some(view) = &self.view {
                    let _ = unsafe { view.Navigate(&HSTRING::from(url)) };
                }
            }
            WebInputEvent::Reload => {
                if let Some(view) = &self.view {
                    let _ = unsafe { view.Reload() };
                }
            }
            WebInputEvent::Focus => self.focus_child(),
            // All pointer input goes through the DevTools protocol as well: it is
            // independent of window handles and focus, which makes it reliable
            // for an offscreen (never activated) browser.
            WebInputEvent::PointerMove { x, y } => {
                let params = format!(
                    r#"{{"type":"mouseMoved","x":{x},"y":{y},"button":"none","buttons":0}}"#
                );
                self.cdp("Input.dispatchMouseEvent", &params);
            }
            WebInputEvent::PointerDown { x, y } => {
                self.focus_child();
                let params = format!(
                    r#"{{"type":"mousePressed","x":{x},"y":{y},"button":"left","buttons":1,"clickCount":1}}"#
                );
                self.cdp("Input.dispatchMouseEvent", &params);
            }
            WebInputEvent::PointerUp { x, y } => {
                let params = format!(
                    r#"{{"type":"mouseReleased","x":{x},"y":{y},"button":"left","buttons":0,"clickCount":1}}"#
                );
                self.cdp("Input.dispatchMouseEvent", &params);
            }
            WebInputEvent::Wheel { x, y, delta } => {
                // Wheel and keyboard go through the DevTools protocol: a hidden,
                // never-activated window does not receive WM_CHAR/WM_MOUSEWHEEL.
                let delta_y = -(f64::from(delta) / 120.0) * 100.0;
                let params = format!(
                    r#"{{"type":"mouseWheel","x":{x},"y":{y},"deltaX":0,"deltaY":{delta_y}}}"#
                );
                self.cdp("Input.dispatchMouseEvent", &params);
            }
            WebInputEvent::Key { vk, down } => {
                let (key, code) = key_meta(vk);
                let params = format!(
                    r#"{{"type":"{}","key":"{key}","code":"{code}","windowsVirtualKeyCode":{vk},"nativeVirtualKeyCode":{vk}}}"#,
                    if down { "rawKeyDown" } else { "keyUp" }
                );
                self.cdp("Input.dispatchKeyEvent", &params);
            }
            WebInputEvent::Char { ch } => {
                let text = serde_json::to_string(&ch.to_string()).unwrap_or_default();
                self.cdp("Input.insertText", &format!(r#"{{"text":{text}}}"#));
            }
        }
        true
    }

    /// Run a DevTools protocol method against the hosted page.
    fn cdp(&self, method: &str, params: &str) {
        let Some(view) = &self.view else {
            return;
        };
        unsafe {
            let _ = view.CallDevToolsProtocolMethod(
                &HSTRING::from(method),
                &HSTRING::from(params),
                None::<&ICoreWebView2CallDevToolsProtocolMethodCompletedHandler>,
            );
        }
    }

    /// Give the render widget real keyboard focus inside this thread's queue.
    fn focus_child(&mut self) {
        if self.child.0.is_null() {
            self.child = find_child_window(self.host).unwrap_or_else(null_hwnd);
        }
        if let Some(controller) = &self.controller {
            let _ = unsafe { controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) };
        }
        if !self.child.0.is_null() {
            unsafe {
                let _ = SetFocus(Some(self.child));
                let _ = PostMessageW(Some(self.child), WM_SETFOCUS, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn find_child_window(host: HWND) -> Option<HWND> {
    if host.0.is_null() {
        return None;
    }
    let mut data = FindChildData {
        ..Default::default()
    };
    data.host = host;
    unsafe {
        let _ = EnumChildWindows(
            Some(host),
            Some(child_callback),
            LPARAM(&mut data as *mut _ as isize),
        );
    }
    if std::env::var_os("VRCX_WEB_DEBUG_HWND").is_some() {
        eprintln!("[web-surface] host={:#x} children:", host.0 as usize);
        for (hwnd, class) in &data.candidates {
            eprintln!("  {:#x}  {class}", hwnd.0 as usize);
        }
    }
    // The render widget host window is the one that accepts input; fall back to
    // the last (deepest) child we saw.
    let mut chosen = data.fallback;
    for (hwnd, class) in &data.candidates {
        if class.contains("RenderWidgetHost") {
            chosen = *hwnd;
            break;
        }
        if class.contains("Chrome_WidgetWin") && chosen.0.is_null() {
            chosen = *hwnd;
        }
    }
    (!chosen.0.is_null()).then_some(chosen)
}

#[derive(Default)]
struct FindChildData {
    fallback: HWND,
    host: HWND,
    candidates: Vec<(HWND, String)>,
}

unsafe extern "system" fn child_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let data = &mut *(lparam.0 as *mut FindChildData);
    let mut buffer = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) } as usize;
    let class = String::from_utf16_lossy(&buffer[..len.min(buffer.len())]);
    data.candidates.push((hwnd, class));
    data.fallback = hwnd;
    BOOL(1) // keep enumerating
}

fn read_stream(stream: &IStream) -> Option<Vec<u8>> {
    unsafe {
        stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
        let mut out = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            // `Read` returns `Result<Infallible, Error>`, so the first `ok()`
            // turns it into an `Option` and the second one applies `?`.
            stream
                .Read(
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                    Some(&mut read),
                )
                .ok()
                .ok()?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        Some(out)
    }
}

fn decode_frame(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    Some((width, height, image.into_raw()))
}

fn create_host_window(cfg: &WebSurfaceConfig) -> Result<HWND, String> {
    unsafe {
        let instance = GetModuleHandleW(None)
            .map_err(|error| format!("GetModuleHandleW failed: {error:?}"))?;
        let hinstance = HINSTANCE(instance.0);
        let class_name = HSTRING::from("Vrcx0WebSurfaceHost");
        let wnd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let atom = RegisterClassW(&wnd_class);
        if atom == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!("RegisterClassW failed: {:?}", GetLastError()));
        }

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            &class_name,
            &HSTRING::from("VRCX-0 Web Overlay"),
            WS_POPUP,
            -10000,
            -10000,
            cfg.width as i32,
            cfg.height as i32,
            None,
            None,
            Some(hinstance),
            None,
        )
        .map_err(|error| format!("CreateWindowExW failed: {error:?}"))?;

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

pub(crate) fn run_engine(cfg: WebSurfaceConfig, shared: Arc<Shared>, rx: Receiver<WebInputEvent>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let host = match create_host_window(&cfg) {
        Ok(host) => host,
        Err(error) => {
            tracing::error!(%error, "web overlay: cannot create host window");
            shared.set_status(WebSurfaceStatus::Failed { message: error });
            return;
        }
    };

    let state = Rc::new(RefCell::new(EngineState {
        cfg: cfg.clone(),
        shared: Arc::clone(&shared),
        controller: None,
        view: None,
        host,
        child: null_hwnd(),
        capturing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        last_capture: Instant::now(),
        ready: false,
    }));
    ENGINE.with(|engine| {
        *engine.borrow_mut() = Some(Rc::clone(&state));
    });

    // 1) create the WebView2 environment
    let state_for_env = Rc::clone(&state);
    let env_handler =
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |hr, environment| {
            if !hr.is_ok() {
                let message = format!("CreateCoreWebView2Environment failed: {hr:?}");
                tracing::error!(%message, "web overlay: environment creation failed");
                state_for_env
                    .borrow()
                    .shared
                    .set_status(WebSurfaceStatus::Failed { message });
                return Ok(());
            }
            let Some(environment) = environment else {
                state_for_env
                    .borrow()
                    .shared
                    .set_status(WebSurfaceStatus::Failed {
                        message: "WebView2 environment is missing".to_string(),
                    });
                return Ok(());
            };
            let state_for_controller = Rc::clone(&state_for_env);
            let controller_handler = CreateCoreWebView2ControllerCompletedHandler::create(
                Box::new(move |hr, controller| {
                    if !hr.is_ok() {
                        let message = format!("CreateCoreWebView2Controller failed: {hr:?}");
                        tracing::error!(%message, "web overlay: controller creation failed");
                        state_for_controller
                            .borrow()
                            .shared
                            .set_status(WebSurfaceStatus::Failed { message });
                        return Ok(());
                    }
                    if let Some(controller) = controller {
                        on_controller_ready(&state_for_controller, controller);
                    }
                    Ok(())
                }),
            );
            let host = state_for_env.borrow().host;
            let _ = unsafe { environment.CreateCoreWebView2Controller(host, &controller_handler) };
            Ok(())
        }));

    let user_data_h = {
        let dir = cfg
            .profile_dir
            .clone()
            .unwrap_or_else(crate::default_profile_dir);
        let _ = crate::ensure_dir(&dir);
        HSTRING::from(dir.to_string_lossy().to_string())
    };
    let user_data_ptr = PCWSTR(user_data_h.as_ptr());

    let result = unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            user_data_ptr,
            None::<&ICoreWebView2EnvironmentOptions>,
            &env_handler,
        )
    };
    if let Err(error) = result {
        let message = format!("CreateCoreWebView2EnvironmentWithOptions failed: {error:?}");
        tracing::error!(%message, "web overlay: cannot start WebView2");
        shared.set_status(WebSurfaceStatus::Failed { message });
        unsafe {
            let _ = DestroyWindow(host);
        }
        unsafe { CoUninitialize() };
        return;
    }

    let _ = unsafe { SetTimer(Some(host), TIMER_TICK, TICK_MS, None) };

    // 2) pump messages; drain input on every iteration
    let mut msg = MSG::default();
    let mut running = true;
    while running {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 == 0 || ret.0 == -1 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    let mut st = state.borrow_mut();
                    if !st.apply_input(event) {
                        unsafe {
                            let _ = DestroyWindow(host);
                        }
                        running = false;
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    unsafe {
                        let _ = DestroyWindow(host);
                    }
                    running = false;
                    break;
                }
            }
        }
        if msg.message == WM_DESTROY {
            break;
        }
    }

    ENGINE.with(|engine| {
        engine.borrow_mut().take();
    });
    unsafe { CoUninitialize() };
    shared.set_status(WebSurfaceStatus::Stopped);
}

fn on_controller_ready(state: &Rc<RefCell<EngineState>>, controller: ICoreWebView2Controller) {
    let view = match unsafe { controller.CoreWebView2() } {
        Ok(view) => view,
        Err(error) => {
            let message = format!("get CoreWebView2 failed: {error:?}");
            tracing::error!(%message, "web overlay: no webview object");
            state
                .borrow()
                .shared
                .set_status(WebSurfaceStatus::Failed { message });
            return;
        }
    };

    let cfg = state.borrow().cfg.clone();
    unsafe {
        let _ = controller.SetIsVisible(true);
        let _ = controller.SetBounds(RECT {
            left: 0,
            top: 0,
            right: cfg.width as i32,
            bottom: cfg.height as i32,
        });
        let _ = controller.SetZoomFactor(cfg.zoom);

        if let Ok(settings) = view.Settings() {
            let _ = settings.SetAreDefaultContextMenusEnabled(false);
            let _ = settings.SetIsStatusBarEnabled(false);
            let _ = settings.SetAreDevToolsEnabled(false);
            let _ = settings.SetIsZoomControlEnabled(true);
            if let Some(user_agent) = &cfg.user_agent {
                if let Ok(settings2) = settings.cast::<ICoreWebView2Settings2>() {
                    let _ = settings2.SetUserAgent(&HSTRING::from(user_agent.clone()));
                }
            }
        }

        // Inject CSS / JS right after every document is created.
        let mut script = String::new();
        // A visible cursor is essential in VR: the page runs offscreen and the
        // user has no system pointer, so we draw one that follows the injected
        // mouse events.
        script.push_str(CURSOR_SCRIPT);
        script.push('\n');
        if let Some(css) = &cfg.inject_css {
            script.push_str("(function(){const s=document.createElement('style');s.textContent=");
            script.push_str(&serde_json::to_string(css).unwrap_or_else(|_| "\"\"".to_string()));
            script.push_str(";document.documentElement.appendChild(s);})();\n");
        }
        if let Some(js) = &cfg.inject_js {
            script.push_str(js);
            script.push('\n');
        }
        if !script.is_empty() {
            let _ = view.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(script), None);
        }

        let _ = view.Navigate(&HSTRING::from(cfg.url.clone()));
        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
    }

    let mut st = state.borrow_mut();
    st.controller = Some(controller);
    st.view = Some(view);
    st.ready = true;
    st.last_capture = Instant::now() - Duration::from_secs(5);
    st.child = find_child_window(st.host).unwrap_or_else(null_hwnd);
    st.shared.set_status(WebSurfaceStatus::Ready);
    tracing::info!(url = %cfg.url, "web overlay: page loading");
}

/// (key, code) for the non-text keys forwarded over the DevTools protocol.
fn key_meta(vk: u16) -> (&'static str, &'static str) {
    match vk {
        8 => ("Backspace", "Backspace"),
        9 => ("Tab", "Tab"),
        13 => ("Enter", "Enter"),
        27 => ("Escape", "Escape"),
        32 => (" ", "Space"),
        33 => ("PageUp", "PageUp"),
        34 => ("PageDown", "PageDown"),
        35 => ("End", "End"),
        36 => ("Home", "Home"),
        37 => ("ArrowLeft", "ArrowLeft"),
        38 => ("ArrowUp", "ArrowUp"),
        39 => ("ArrowRight", "ArrowRight"),
        40 => ("ArrowDown", "ArrowDown"),
        46 => ("Delete", "Delete"),
        _ => ("Unidentified", ""),
    }
}

/// Cursor overlay injected into every page: a ring that follows injected mouse
/// events, plus a click flash. Without it the VR user cannot see where the
/// pointer is (the page is rendered offscreen).
const CURSOR_SCRIPT: &str = r#"
(function () {
  if (window.__vrcxCursor) return;
  window.__vrcxCursor = true;
  function install() {
    var c = document.createElement('div');
    c.id = '__vrcx_cursor';
    c.style.cssText = 'position:fixed;left:-100px;top:-100px;width:24px;height:24px;' +
      'margin:-12px 0 0 -12px;border-radius:50%;border:3px solid #fff;box-sizing:border-box;' +
      'box-shadow:0 0 0 2px rgba(0,0,0,.5),0 0 14px rgba(124,156,255,.95);' +
      'background:rgba(124,156,255,.35);pointer-events:none;z-index:2147483647;' +
      'transition:transform .08s ease-out;display:none';
    (document.body || document.documentElement).appendChild(c);
    function move(e) {
      c.style.display = 'block';
      c.style.left = (e.clientX || 0) + 'px';
      c.style.top = (e.clientY || 0) + 'px';
    }
    function flash() {
      c.style.transform = 'scale(1.6)';
      setTimeout(function () { c.style.transform = ''; }, 130);
    }
    document.addEventListener('mousemove', move, true);
    document.addEventListener('mousedown', function (e) { move(e); flash(); }, true);
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', install);
  } else {
    install();
  }
})();
"#;
