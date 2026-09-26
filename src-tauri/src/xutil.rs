//! X11 辅助: 让键盘/悬浮球窗口不抢焦点 (WM_HINTS input=False + UTILITY 类型),
//! 并提供全局光标位置查询 (用于点击穿透判断)。
//! 仅在 X11/XWayland 环境生效; 拿不到 X 连接时静默跳过。

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as XprotoExt, PropMode};
use x11rb::wrapper::ConnectionExt as WrapperExt;

pub struct XConn {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    root_w: i32,
    root_h: i32,
    utf8: u32,
    net_wm_name: u32,
    net_wm_type: u32,
    type_utility: u32,
}

impl XConn {
    pub fn new() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[screen].root;
        let root_w = conn.setup().roots[screen].width_in_pixels as i32;
        let root_h = conn.setup().roots[screen].height_in_pixels as i32;
        let utf8 = conn
            .intern_atom(false, b"UTF8_STRING")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        let net_wm_name = conn
            .intern_atom(false, b"_NET_WM_NAME")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        let net_wm_type = conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        let type_utility = conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE_UTILITY")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        Ok(Self { conn, root, root_w, root_h, utf8, net_wm_name, net_wm_type, type_utility })
    }

    /// root 窗口像素尺寸 (uinput 画笔绝对定位的归一化基准)
    pub fn root_size(&self) -> (i32, i32) {
        (self.root_w, self.root_h)
    }

    fn window_name(&self, w: u32) -> Option<String> {        let net = self
            .conn
            .get_property(false, w, self.net_wm_name, self.utf8, 0, 512)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.value);
        if let Some(v) = net {
            if !v.is_empty() {
                return Some(String::from_utf8_lossy(&v).into_owned());
            }
        }
        let wm = self
            .conn
            .get_property(false, w, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 512)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.value)?;
        if wm.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&wm).into_owned())
        }
    }

    fn walk(&self, w: u32, depth: usize, found: &mut Vec<u32>, titles: &[&str]) -> Result<(), String> {
        if depth > 6 {
            return Ok(());
        }
        if let Some(name) = self.window_name(w) {
            if titles.iter().any(|t| name == *t) && !found.contains(&w) {
                found.push(w);
            }
        }
        let reply = self.conn.query_tree(w).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        for c in reply.children {
            self.walk(c, depth + 1, found, titles)?;
        }
        Ok(())
    }

    /// 给指定标题的窗口设置 input=False 与 UTILITY 类型, 返回处理数量。
    pub fn relax_focus(&self, titles: &[&str]) -> Result<usize, String> {
        let mut found = Vec::new();
        self.walk(self.root, 0, &mut found, titles)?;
        for &w in &found {
            // WM_HINTS: flags=InputHint(1), input=False, 其余为 0
            let hints: [u32; 9] = [1, 0, 0, 0, 0, 0, 0, 0, 0];
            self.conn
                .change_property32(PropMode::REPLACE, w, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, &hints)
                .map_err(|e| e.to_string())?;
            self.conn
                .change_property32(PropMode::REPLACE, w, self.net_wm_type, AtomEnum::ATOM, &[self.type_utility])
                .map_err(|e| e.to_string())?;
        }
        self.conn.flush().map_err(|e| e.to_string())?;
        Ok(found.len())
    }

    /// 按标题查找本应用的 X 窗口 id (仅匹配映射过的托盘/键盘/悬浮球窗口)
    pub fn find_window_id(&self, title: &str) -> Option<u32> {
        let mut found = Vec::new();
        let _ = self.walk(self.root, 0, &mut found, &[title]);
        found.first().copied()
    }

    /// 设置 XShape 输入区域 (物理像素矩形列表): 区域内的事件进窗口, 区域外穿透。
    /// 相比轮询 set_ignore_cursor_events, 这是服务端即时生效的静态区域, 无延迟无竞争。
    pub fn set_input_shape(&self, window: u32, rects: &[(f64, f64, f64, f64)], scale: f64) -> Result<(), String> {
        use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
        let xrects: Vec<x11rb::protocol::xproto::Rectangle> = rects
            .iter()
            .map(|(x, y, w, h)| x11rb::protocol::xproto::Rectangle {
                x: (x * scale).round() as i16,
                y: (y * scale).round() as i16,
                width: (w * scale).round() as u16,
                height: (h * scale).round() as u16,
            })
            .collect();
        self.conn
            .shape_rectangles(SO::SET, SK::INPUT, x11rb::protocol::xproto::ClipOrdering::UNSORTED, window, 0, 0, &xrects)
            .map_err(|e| e.to_string())?;
        self.conn.flush().map_err(|e| e.to_string())
    }

    /// 恢复整个窗口可交互 (清除输入区域限制)
    pub fn clear_input_shape(&self, window: u32) -> Result<(), String> {
        use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
        let full = [x11rb::protocol::xproto::Rectangle { x: -1, y: -1, width: 20000, height: 20000 }];
        self.conn
            .shape_rectangles(SO::SET, SK::INPUT, x11rb::protocol::xproto::ClipOrdering::UNSORTED, window, 0, 0, &full)
            .map_err(|e| e.to_string())?;
        self.conn.flush().map_err(|e| e.to_string())
    }

    fn intern(&self, name: &str) -> Option<u32> {
        self.conn
            .intern_atom(false, name.as_bytes())
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    }

    /// 当前桌面索引
    pub fn current_desktop(&self) -> Option<u32> {
        let atom = self.intern("_NET_CURRENT_DESKTOP")?;
        let r = self
            .conn
            .get_property(false, self.root, atom, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        let v: Vec<u32> = r.value32()?.collect();
        v.first().copied()
    }

    /// X11 工作区 (扣除 dock/面板后的可用区域, 全局物理坐标)。
    /// 由窗口管理器 (mutter/kwin 等) 维护, 反映 dock 栏实际占据的空间。
    pub fn workarea(&self) -> Option<(i32, i32, u32, u32)> {
        let atom = self.intern("_NET_WORKAREA")?;
        let r = self
            .conn
            .get_property(false, self.root, atom, AtomEnum::CARDINAL, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        let v: Vec<u32> = r.value32()?.collect();
        let desk = self.current_desktop().unwrap_or(0) as usize;
        let base = desk * 4;
        if v.len() >= base + 4 {
            Some((v[base] as i32, v[base + 1] as i32, v[base + 2], v[base + 3]))
        } else {
            None
        }
    }

    /// 全局光标坐标 (X root 坐标, 物理像素)。XWayland 会将触摸位置同步到核心指针,
    /// 因此鼠标与触摸均可由此获得全局坐标 (悬浮球拖动依赖)。
    pub fn cursor_pos(&self) -> Result<(f64, f64), String> {
        let r = self.conn.query_pointer(self.root).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        Ok((r.root_x as f64, r.root_y as f64))
    }
}
