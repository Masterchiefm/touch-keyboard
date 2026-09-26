//! uinput (内核虚拟键盘) 按键注入。X11 与 Wayland 会话通用,
//! 前提是 /dev/uinput 可写 (udev 规则或加入 input 组, 见 README)。
//! 注意: uinput 按物理位置发码, 假定系统使用美式 QWERTY 布局。

use super::InputBackend;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::thread::sleep;
use std::time::Duration;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;

const KEY_LEFTSHIFT: u16 = 42;
const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTALT: u16 = 56;
const KEY_LEFTMETA: u16 = 125;

const fn iow(ty: u8, nr: u8, size: usize) -> libc::c_ulong {
    ((1u64 << 30) | ((size as u64) << 16) | ((ty as u64) << 8) | nr as u64) as libc::c_ulong
}
const UI_SET_EVBIT: libc::c_ulong = iow(b'U', 100, 4);
const UI_SET_KEYBIT: libc::c_ulong = iow(b'U', 101, 4);
const UI_DEV_CREATE: libc::c_ulong = 0x5501;

/// 逻辑键名 -> Linux EV_KEY 键码
fn key_code(code: &str) -> Option<u16> {
    Some(match code {
        "space" => 57,
        "enter" | "return" => 28,
        "backspace" => 14,
        "tab" => 15,
        "escape" | "esc" => 1,
        "left" => 105,
        "up" => 103,
        "right" => 106,
        "down" => 108,
        "home" => 102,
        "end" => 107,
        "delete" => 111,
        "super" => 125,
        _ => {
            let mut it = code.chars();
            let c = it.next()?;
            if it.next().is_some() {
                return None;
            }
            match c {
                'a'..='z' => [
                    30u16, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,
                ][(c as u8 - b'a') as usize],
                '1'..='9' => c as u16 - '1' as u16 + 2,
                '0' => 11,
                '-' => 12,
                '=' => 13,
                '[' => 26,
                ']' => 27,
                '\\' => 43,
                ';' => 39,
                '\'' => 40,
                '`' => 41,
                ',' => 51,
                '.' => 52,
                '/' => 53,
                _ => return None,
            }
        }
    })
}

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    name: [u8; 80],
    id: InputId,
    ff_effects_max: u32,
    absmax: [i32; 64],
    absmin: [i32; 64],
    absfuzz: [i32; 64],
    absflat: [i32; 64],
}

#[repr(C)]
struct InputEvent {
    tv_sec: i64,
    tv_usec: i64,
    typ: u16,
    code: u16,
    value: i32,
}

pub struct UinputBackend {
    f: File,
}

impl UinputBackend {
    pub fn new() -> Result<Self, String> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
            .map_err(|e| {
                format!(
                    "无法打开 /dev/uinput: {e} (需要权限: 安装仓库内 udev 规则或将用户加入 input 组, 见 README)"
                )
            })?;
        let fd = f.as_raw_fd();
        unsafe {
            for (req, val) in [(UI_SET_EVBIT, EV_KEY as i32), (UI_SET_EVBIT, EV_SYN as i32)] {
                if libc::ioctl(fd, req, val) < 0 {
                    return Err("UI_SET_EVBIT 失败".into());
                }
            }
            for c in 1..=248u32 {
                libc::ioctl(fd, UI_SET_KEYBIT, c as i32);
            }
        }
        let mut setup = UinputSetup {
            name: [0; 80],
            id: InputId { bustype: 0x03, vendor: 0x1234, product: 0x5678, version: 1 },
            ff_effects_max: 0,
            absmax: [0; 64],
            absmin: [0; 64],
            absfuzz: [0; 64],
            absflat: [0; 64],
        };
        setup.name[..13].copy_from_slice(b"TouchKeyboard");
        let written = unsafe {
            libc::write(fd, &setup as *const _ as *const libc::c_void, std::mem::size_of::<UinputSetup>())
        };
        if written < 0 {
            return Err("写入 uinput 设备配置失败".into());
        }
        unsafe {
            if libc::ioctl(fd, UI_DEV_CREATE) < 0 {
                return Err("UI_DEV_CREATE 失败".into());
            }
        }
        Ok(Self { f })
    }

    fn emit(&self, typ: u16, code: u16, value: i32) {
        let ev = InputEvent { tv_sec: 0, tv_usec: 0, typ, code, value };
        let bytes = unsafe {
            std::slice::from_raw_parts(&ev as *const _ as *const u8, std::mem::size_of::<InputEvent>())
        };
        let _ = (&self.f).write_all(bytes);
    }

    fn key(&self, code: u16, down: bool) {
        self.emit(EV_KEY, code, if down { 1 } else { 0 });
        self.emit(EV_SYN, 0, 0);
        let _ = (&self.f).flush();
    }
}

/// 写一条输入事件到 /dev/uinput (触摸板鼠标后端共用)。
/// 经 libc::write: &File 无可变借用, 与设备创建路径同一写法。
fn emit_event(f: &File, typ: u16, code: u16, value: i32) {
    let ev = InputEvent { tv_sec: 0, tv_usec: 0, typ, code, value };
    let bytes = unsafe {
        std::slice::from_raw_parts(&ev as *const _ as *const u8, std::mem::size_of::<InputEvent>())
    };
    unsafe {
        libc::write(f.as_raw_fd(), bytes.as_ptr() as *const libc::c_void, bytes.len());
    }
}

// ---------- 触摸板: uinput 虚拟鼠标 (相对移动 + 三键 + 滚轮) ----------

const EV_REL: u16 = 0x02;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;
const BTN_MOUSE_L: u16 = 0x110; // BTN_LEFT / _RIGHT=+1 / _MIDDLE=+2
const BTN_MOUSE_R: u16 = 0x111;
const BTN_MOUSE_M: u16 = 0x112;
const UI_SET_RELBIT: libc::c_ulong = iow(b'U', 102, 4);

pub struct UinputMouse {
    f: File,
}

impl UinputMouse {
    pub fn new() -> Result<Self, String> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
            .map_err(|e| format!("无法打开 /dev/uinput: {e} (触摸板需要 input 组权限, 见 README)"))?;
        let fd = f.as_raw_fd();
        unsafe {
            for (req, val) in [
                (UI_SET_EVBIT, EV_KEY as i32),
                (UI_SET_EVBIT, EV_REL as i32),
                (UI_SET_EVBIT, EV_SYN as i32),
            ] {
                if libc::ioctl(fd, req, val) < 0 {
                    return Err("UI_SET_EVBIT 失败".into());
                }
            }
            for c in [BTN_MOUSE_L, BTN_MOUSE_R, BTN_MOUSE_M] {
                if libc::ioctl(fd, UI_SET_KEYBIT, c as i32) < 0 {
                    return Err("UI_SET_KEYBIT 失败".into());
                }
            }
            for r in [REL_X, REL_Y, REL_HWHEEL, REL_WHEEL] {
                if libc::ioctl(fd, UI_SET_RELBIT, r as i32) < 0 {
                    return Err("UI_SET_RELBIT 失败".into());
                }
            }
        }
        let mut setup = UinputSetup {
            name: [0; 80],
            id: InputId { bustype: 0x03, vendor: 0x1234, product: 0x5679, version: 1 },
            ff_effects_max: 0,
            absmax: [0; 64],
            absmin: [0; 64],
            absfuzz: [0; 64],
            absflat: [0; 64],
        };
        setup.name[..22].copy_from_slice(b"TouchKeyboard Touchpad");
        let written = unsafe {
            libc::write(fd, &setup as *const _ as *const libc::c_void, std::mem::size_of::<UinputSetup>())
        };
        if written < 0 {
            return Err("写入 uinput 设备配置失败".into());
        }
        unsafe {
            if libc::ioctl(fd, UI_DEV_CREATE) < 0 {
                return Err("UI_DEV_CREATE 失败".into());
            }
        }
        Ok(Self { f })
    }

    fn syn(&self) {
        emit_event(&self.f, EV_SYN, 0, 0);
        let _ = (&self.f).flush();
    }
}

impl super::mouse::MouseBackend for UinputMouse {
    fn name(&self) -> &'static str {
        "uinput"
    }

    fn move_rel(&self, dx: f64, dy: f64) -> Result<(), String> {
        if dx == 0.0 && dy == 0.0 {
            return Ok(());
        }
        emit_event(&self.f, EV_REL, REL_X, dx.round() as i32);
        emit_event(&self.f, EV_REL, REL_Y, dy.round() as i32);
        self.syn();
        Ok(())
    }

    fn button(&self, btn: u8, press: bool) -> Result<(), String> {
        let code = match btn {
            1 => BTN_MOUSE_L,
            2 => BTN_MOUSE_M,
            3 => BTN_MOUSE_R,
            _ => return Err(format!("未知鼠标键: {btn}")),
        };
        emit_event(&self.f, EV_KEY, code, if press { 1 } else { 0 });
        self.syn();
        Ok(())
    }

    fn warp(&self, _x: i64, _y: i64, _sw: i32, _sh: i32) -> Result<(), String> {
        Err("uinput 无绝对定位 (mutter 不转发 XTEST 移动、忽略虚拟画笔), 走阻尼伺服".into())
    }

    fn scroll(&self, dx: i32, dy: i32) -> Result<(), String> {
        // 内核约定: REL_WHEEL 正=向上, REL_HWHEEL 正=向右
        for _ in 0..dy.abs().min(30) {
            emit_event(&self.f, EV_REL, REL_WHEEL, if dy > 0 { -1 } else { 1 });
            self.syn();
        }
        for _ in 0..dx.abs().min(30) {
            emit_event(&self.f, EV_REL, REL_HWHEEL, if dx > 0 { 1 } else { -1 });
            self.syn();
        }
        Ok(())
    }

}

impl InputBackend for UinputBackend {
    fn name(&self) -> &'static str {
        "uinput"
    }

    fn tap(&self, code: &str, shift: bool, ctrl: bool, alt: bool, meta: bool) -> Result<(), String> {
        let kc = key_code(code).ok_or_else(|| format!("未知按键: {code}"))?;
        if ctrl {
            self.key(KEY_LEFTCTRL, true);
        }
        if alt {
            self.key(KEY_LEFTALT, true);
        }
        if meta {
            self.key(KEY_LEFTMETA, true);
        }
        if shift {
            self.key(KEY_LEFTSHIFT, true);
        }
        sleep(Duration::from_millis(4));
        self.key(kc, true);
        sleep(Duration::from_millis(15));
        self.key(kc, false);
        if shift {
            self.key(KEY_LEFTSHIFT, false);
        }
        if meta {
            self.key(KEY_LEFTMETA, false);
        }
        if alt {
            self.key(KEY_LEFTALT, false);
        }
        if ctrl {
            self.key(KEY_LEFTCTRL, false);
        }
        Ok(())
    }
}
