//! 触摸板鼠标注入: 相对移动 / 按键 / 滚轮。
//! 后端选择与键盘一致 (input::create_backend):
//! - X11 会话优先 XTEST (无需额外权限; XWayland 下 mutter 会转发 XTEST 指针事件);
//! - Wayland 会话优先 uinput (内核级虚拟鼠标, 与真实鼠标无异, 可到达所有窗口);
//! - TOUCH_KB_BACKEND=x11|uinput 同样对鼠标生效。

use x11rb::connection::Connection;
use x11rb::protocol::xtest::ConnectionExt as _;

/// 鼠标按键编号约定 (与 X11 一致): 1=左 2=中 3=右
pub trait MouseBackend: Send {
    fn name(&self) -> &'static str;
    /// 相对移动 (物理像素, 可为小数, 由后端取整)
    fn move_rel(&self, dx: f64, dy: f64) -> Result<(), String>;
    /// 按下/抬起鼠标按键 (1=左 2=中 3=右)
    fn button(&self, btn: u8, press: bool) -> Result<(), String>;
    /// 滚轮 (整数格数: dy 正=向下, dx 正=向右)
    fn scroll(&self, dx: i32, dy: i32) -> Result<(), String>;
    /// 绝对定位光标到 root 物理坐标。仅 XTEST 后端 (真 X11 会话) 支持;
    /// uinput 无绝对定位 (mutter 不转发 XTEST 移动、忽略虚拟画笔, 已实测),
    /// 返回 Err 由调用方走阻尼相对移动伺服 (tp_step_toward_v)。
    fn warp(&self, x: i64, y: i64, screen_w: i32, screen_h: i32) -> Result<(), String>;
}

// ---------- XTEST 后端 (X11 会话) ----------

pub struct XtestMouse {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
}

impl XtestMouse {
    pub fn new() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 连接失败: {e}"))?;
        let root = conn.setup().roots[screen].root;
        Ok(Self { conn, root })
    }

    fn fake(&self, ty: u8, detail: u8, x: i16, y: i16) -> Result<(), String> {
        // XTEST FakeInput: 4/5 = 鼠标按下/抬起 (detail=按键号), 6 = 移动 (detail=1 相对, root=None)
        self.conn
            .xtest_fake_input(ty, detail, 0, 0, x, y, 0)
            .map_err(|e| e.to_string())?;
        self.conn.flush().map_err(|e| e.to_string())
    }
}

impl MouseBackend for XtestMouse {
    fn name(&self) -> &'static str {
        "x11-xtest"
    }

    fn move_rel(&self, dx: f64, dy: f64) -> Result<(), String> {
        if dx == 0.0 && dy == 0.0 {
            return Ok(());
        }
        self.fake(6, 1, dx.round() as i16, dy.round() as i16)
    }

    fn button(&self, btn: u8, press: bool) -> Result<(), String> {
        if !(1..=3).contains(&btn) {
            return Err(format!("未知鼠标键: {btn}"));
        }
        self.fake(if press { 4 } else { 5 }, btn, 0, 0)
    }

    fn scroll(&self, dx: i32, dy: i32) -> Result<(), String> {
        // X11 滚轮 = 虚拟按键: 4/5 = 上/下, 6/7 = 左/右
        let mut fires: Vec<(u8, i32)> = Vec::new();
        if dy != 0 {
            fires.push((if dy > 0 { 5 } else { 4 }, dy.abs().min(30)));
        }
        if dx != 0 {
            fires.push((if dx > 0 { 7 } else { 6 }, dx.abs().min(30)));
        }
        for (btn, n) in fires {
            for _ in 0..n {
                self.fake(4, btn, 0, 0)?;
                self.fake(5, btn, 0, 0)?;
            }
        }
        Ok(())
    }

    fn warp(&self, x: i64, y: i64, _sw: i32, _sh: i32) -> Result<(), String> {
        // XTEST 绝对移动 (type=6 detail=0, 相对 root 的坐标)。真 X11 会话生效;
        // Wayland 会话 mutter 不转发 XTEST 移动, 但那里默认后端是 uinput
        self.conn
            .xtest_fake_input(6, 0, 0, self.root, x as i16, y as i16, 0)
            .map_err(|e| e.to_string())?;
        self.conn.flush().map_err(|e| e.to_string())
    }
}

/// 按环境选择鼠标后端 (逻辑与 create_backend 一致)
pub fn create_mouse_backend() -> Result<Box<dyn MouseBackend + Send>, String> {
    use super::uinput::UinputMouse;

    let forced = std::env::var("TOUCH_KB_BACKEND").unwrap_or_default().to_lowercase();
    let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    match forced.as_str() {
        "x11" => return Ok(Box::new(XtestMouse::new()?)),
        "uinput" => return Ok(Box::new(UinputMouse::new()?)),
        _ => {}
    }

    let mut errs = Vec::new();
    let prefer_uinput = wayland;
    for use_uinput in [prefer_uinput, !prefer_uinput] {
        let r = if use_uinput {
            UinputMouse::new().map(|b| Box::new(b) as Box<dyn MouseBackend + Send>)
        } else {
            XtestMouse::new().map(|b| Box::new(b) as Box<dyn MouseBackend + Send>)
        };
        match r {
            Ok(b) => return Ok(b),
            Err(e) => errs.push(format!("{}: {e}", if use_uinput { "uinput" } else { "x11" })),
        }
    }
    Err(format!("无可用鼠标后端: {}", errs.join("; ")))
}
