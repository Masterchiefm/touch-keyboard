//! X11 XTEST 按键注入。通过读取服务器键码映射 (keysym -> keycode) 支持任意布局,
//! 含 Shift 与 AltGr (ISO_Level3_Shift) 层级。

use super::InputBackend;
use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xtest::ConnectionExt as _;

const SHIFT_L: u32 = 0xFFE1;
const CTRL_L: u32 = 0xFFE3;
const ALT_L: u32 = 0xFFE9;
const SUPER_L: u32 = 0xFFEB;
const LEVEL3: u32 = 0xFE03;

fn keysym_of(code: &str) -> Option<u32> {
    Some(match code {
        "space" => 0x20,
        "enter" | "return" => 0xFF0D,
        "backspace" => 0xFF08,
        "tab" => 0xFF09,
        "escape" | "esc" => 0xFF1B,
        "left" => 0xFF51,
        "up" => 0xFF52,
        "right" => 0xFF53,
        "down" => 0xFF54,
        "home" => 0xFF50,
        "end" => 0xFF57,
        "delete" => 0xFFFF,
        "super" => SUPER_L,
        _ => {
            let mut it = code.chars();
            let c = it.next()?;
            if it.next().is_none() && c.is_ascii() {
                c as u32
            } else {
                return None;
            }
        }
    })
}

pub struct X11Backend {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    keymap: HashMap<u32, (u8, u8)>, // keysym -> (keycode, level)
}

impl X11Backend {
    pub fn new() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 连接失败: {e}"))?;
        let root = conn.setup().roots[screen].root;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let count = max.saturating_sub(min).saturating_add(1);
        let reply = conn
            .get_keyboard_mapping(min, count)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let per = reply.keysyms_per_keycode as usize;
        if per == 0 {
            return Err("键盘映射格式异常".into());
        }
        let mut keymap = HashMap::new();
        for (i, chunk) in reply.keysyms.chunks(per).enumerate() {
            let kc = min + i as u8;
            for (level, &ks) in chunk.iter().enumerate() {
                if ks == 0 || level > 3 {
                    continue;
                }
                keymap.entry(ks).or_insert((kc, level as u8));
            }
        }
        if keymap.is_empty() {
            return Err("键盘映射为空".into());
        }
        Ok(Self { conn, root, keymap })
    }

    fn fake(&self, press: bool, kc: u8) -> Result<(), String> {
        // XTEST FakeInput: type 2 = KeyPress, 3 = KeyRelease
        self.conn
            .xtest_fake_input(if press { 2 } else { 3 }, kc, 0, self.root, 0, 0, 0)
            .map_err(|e| e.to_string())?;
        self.conn.flush().map_err(|e| e.to_string())
    }

    fn modifier_kc(&self, ks: u32) -> Result<u8, String> {
        self.keymap
            .get(&ks)
            .map(|x| x.0)
            .ok_or_else(|| format!("当前布局缺少修饰键 keysym {ks:#x}"))
    }
}

impl InputBackend for X11Backend {
    fn name(&self) -> &'static str {
        "x11-xtest"
    }

    fn tap(&self, code: &str, shift: bool, ctrl: bool, alt: bool, meta: bool) -> Result<(), String> {
        let ks = keysym_of(code).ok_or_else(|| format!("未知按键: {code}"))?;
        let (kc, level) = *self
            .keymap
            .get(&ks)
            .ok_or_else(|| format!("按键 {code} 不在当前 X11 键盘布局中"))?;
        let need_shift = shift || level % 2 == 1;
        let need_level3 = level >= 2;

        if ctrl {
            self.fake(true, self.modifier_kc(CTRL_L)?)?;
        }
        if alt {
            self.fake(true, self.modifier_kc(ALT_L)?)?;
        }
        if meta {
            self.fake(true, self.modifier_kc(SUPER_L)?)?;
        }
        if need_level3 {
            self.fake(true, self.modifier_kc(LEVEL3)?)?;
        }
        if need_shift {
            self.fake(true, self.modifier_kc(SHIFT_L)?)?;
        }
        sleep(Duration::from_millis(6));
        self.fake(true, kc)?;
        sleep(Duration::from_millis(20));
        self.fake(false, kc)?;
        if need_shift {
            self.fake(false, self.modifier_kc(SHIFT_L)?)?;
        }
        if need_level3 {
            self.fake(false, self.modifier_kc(LEVEL3)?)?;
        }
        if meta {
            self.fake(false, self.modifier_kc(SUPER_L)?)?;
        }
        if alt {
            self.fake(false, self.modifier_kc(ALT_L)?)?;
        }
        if ctrl {
            self.fake(false, self.modifier_kc(CTRL_L)?)?;
        }
        Ok(())
    }
}
