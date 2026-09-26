pub mod mouse;
pub mod uinput;
pub mod x11;

/// 按键注入后端抽象: 一个 "tap" = 一次完整按键 (可带修饰键)。
pub trait InputBackend: Send {
    fn name(&self) -> &'static str;
    fn tap(&self, code: &str, shift: bool, ctrl: bool, alt: bool, meta: bool) -> Result<(), String>;
}

/// 按环境选择后端:
/// - X11 会话优先 XTEST (无需额外权限), 失败回退 uinput;
/// - Wayland 会话优先 uinput (内核级注入, 可到达原生 Wayland 与 XWayland 所有窗口),
///   XTEST 仅能到达 XWayland 窗口, 因此只作回退;
/// - 可用环境变量 TOUCH_KB_BACKEND=x11|uinput 强制指定。
pub fn create_backend() -> Result<Box<dyn InputBackend + Send>, String> {
    let forced = std::env::var("TOUCH_KB_BACKEND").unwrap_or_default().to_lowercase();
    let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    match forced.as_str() {
        "x11" => {
            return x11::X11Backend::new()
                .map(|b| Box::new(b) as Box<dyn InputBackend + Send>)
                .map_err(|e| format!("x11: {e}"))
        }
        "uinput" => {
            return uinput::UinputBackend::new()
                .map(|b| Box::new(b) as Box<dyn InputBackend + Send>)
                .map_err(|e| format!("uinput: {e}"))
        }
        _ => {}
    }

    let mut errs = Vec::new();
    if wayland {
        match uinput::UinputBackend::new() {
            Ok(b) => return Ok(Box::new(b)),
            Err(e) => errs.push(format!("uinput: {e}")),
        }
        match x11::X11Backend::new() {
            Ok(b) => return Ok(Box::new(b)),
            Err(e) => errs.push(format!("x11: {e}")),
        }
    } else {
        match x11::X11Backend::new() {
            Ok(b) => return Ok(Box::new(b)),
            Err(e) => errs.push(format!("x11: {e}")),
        }
        match uinput::UinputBackend::new() {
            Ok(b) => return Ok(Box::new(b)),
            Err(e) => errs.push(format!("uinput: {e}")),
        }
    }
    Err(format!("无可用输入后端: {}", errs.join("; ")))
}
