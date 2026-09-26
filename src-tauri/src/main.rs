#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod input;
mod updater;
mod xutil;

use input::mouse::MouseBackend;
use input::InputBackend;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, WebviewWindow, WindowEvent};

const KB_ASPECT: f64 = 0.375; // 完整键盘 高/宽 比 (参照 Windows 屏幕键盘: 键帽近正方, 高度随窗口宽度变化)
const SPLIT_HALF_UNITS: f64 = 6.8; // 分裂半面板宽度 (含边距) 折合键距数, 用于推算行高
const BALL_SIZE: f64 = 56.0;
const TP_WIN_W: f64 = 440.0; // 触摸板窗口默认逻辑尺寸 (与 tauri.conf.json 一致)
const TP_WIN_H: f64 = 400.0;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    mode: String,      // "full" | "split"
    kb_scale: f64,     // 0.7 - 1.5 用户缩放
    opacity: f64,      // 0.3 - 1.0
    num_row: bool,     // 数字行
    click_through: bool,
    float_mode: bool,         // 悬浮窗口模式 (否 = 停靠底部整行)
    kb_pos: Option<[i32; 2]>, // 悬浮位置 (全局物理像素, 窗口左上角); None = 尚未确定, refit 按锚点推算
    float_width: f64,         // 悬浮宽度占工作区宽度比例 0.3 - 0.9
    split_half_units: f64,    // 分裂半面板宽度 (折合键距数 5.0-9.5); 纯前端布局值, 后端透传回显
    theme: String,            // "light" | "dark" | "auto" (auto = 跟随系统深浅色, 默认)
    bubble: bool,             // 按键气泡: 按下时在键上方放大显示所按内容
    tp_speed: f64,            // 触摸板指针速度倍率 0.3 - 3.0
    tp_tap_click: bool,       // 触摸板轻触点击 (单指左键/双指右键/三指中键)
    tp_natural_scroll: bool,  // 触摸板自然滚动 (内容跟随手指)
    tp_pos: Option<[i32; 2]>, // 触摸板窗口位置 (全局物理像素, Moved 事件更新)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: "full".into(),
            kb_scale: 1.0,
            opacity: 0.95,
            num_row: true,
            click_through: true,
            float_mode: false,
            kb_pos: None,
            float_width: 0.6,
            split_half_units: SPLIT_HALF_UNITS,
            theme: "auto".into(),
            bubble: true,
            tp_speed: 1.0,
            tp_tap_click: true,
            tp_natural_scroll: true,
            tp_pos: None,
        }
    }
}

#[derive(Serialize, Clone)]
struct ScreenInfo {
    logical_width: f64,
    logical_height: f64,
    scale_factor: f64,
    // 工作区 (扣除 dock/面板, 全局逻辑坐标); None 表示不可用
    workarea: Option<[f64; 4]>,
}

struct AppState {
    input: Arc<Mutex<Option<Box<dyn InputBackend + Send>>>>,
    mouse: Arc<Mutex<Option<Box<dyn MouseBackend + Send>>>>, // 触摸板鼠标注入后端
    tray_tp: Mutex<Option<CheckMenuItem<tauri::Wry>>>,        // 托盘「触摸板」勾选项 (同步显示状态)
    hit_regions: Mutex<HashMap<String, Vec<[f64; 4]>>>, // 窗口label -> CSS 像素 [x, y, w, h]
    click_through: AtomicBool,
    settings: Mutex<Settings>,
    ball_pos: Mutex<Option<(i32, i32)>>,
    ball_corner: Mutex<(bool, bool)>, // 球体在窗口内的锚定角 (h_right, v_bottom)
    cursor_tracking: AtomicBool,  // X11 可用: 悬浮球拖动走 Rust 全局坐标轮询
    ball_dragging: AtomicBool,    // JS pointerdown/up 置位
    ball_drag_active: AtomicBool, // 拖动线程已进入跟随状态
    kb_dragging: AtomicBool,      // 键盘拖动把手: JS pointerdown/up 置位
    kb_drag_active: AtomicBool,   // 键盘拖动线程已进入跟随状态
    xconn: Mutex<Option<xutil::XConn>>,
    xids: Mutex<HashMap<String, u32>>, // 窗口label -> X window id
    kb_float_w: Mutex<f64>,       // 上次 refit 应用的悬浮窗口物理宽度 (0 = 未知), 宽度变化时作中心锚定基准
    ime_label: Mutex<String>,     // 当前输入法短标签 (轮询 ibus/fcitx5, 随托盘指示同步)
    tp_vpos: Mutex<Option<(f64, f64)>>, // 触摸板虚拟指针位置 V (root 物理坐标; 空闲时跟随真实光标)
    tp_fingers: AtomicU8,         // 触摸板表面当前触点数 (JS touchstart/end 上报)
    tp_js_motion: AtomicBool,     // 本轮手势 JS 已发过 tp_move (WebKitGTK 派发了 touchmove)
    tp_travel: AtomicU32,         // 本轮手势累计行程 px (轻触判定)
    tp_gain: Mutex<f64>,          // 本轮手势增益 (工作区宽/板宽 × tp_speed, tp_surface 时刷新)
    update: Mutex<UpdateMem>,     // 新版本检测 (updater.rs): 最新 Release 与并发门旗
}

/// 更新检查的内存态 (不落盘: 每次启动都检查一次, 无需跨启动记忆检查时间)
#[derive(Default)]
struct UpdateMem {
    /// 上次成功查到 Release 的时间 (网络失败不记, 下个小时仍会重试)
    last_check_ms: i64,
    checking: bool,
    /// 发现的新版本 (Some 即有更新; 前端经 update-available 事件得知)
    latest: Option<updater::Release>,
}

/// 借用缓存的 X 连接执行操作
fn with_xconn<T>(app: &AppHandle, f: impl FnOnce(&xutil::XConn) -> Option<T>) -> Option<T> {
    let state = app.state::<AppState>();
    let mut guard = state.xconn.lock().ok()?;
    if guard.is_none() {
        *guard = xutil::XConn::new().ok();
    }
    f(guard.as_ref()?)
}

/// 把窗口的输入区域设置为上报的可交互区域 (按键并集/球体)。
/// XShape 在服务端即时生效: 区域内事件进窗口, 区域外穿透 —— 无轮询、无延迟、无竞争。
fn apply_input_shape(app: &AppHandle, win: &str) -> Option<()> {
    let state = app.state::<AppState>();
    let rects: Option<Vec<(f64, f64, f64, f64)>> = state
        .hit_regions
        .lock()
        .ok()
        .and_then(|m| m.get(win).cloned())
        .map(|rs| rs.iter().map(|r| (r[0], r[1], r[2], r[3])).collect());
    // xid 未缓存时按需查找 (悬浮球的 X 窗口首次显示时才创建)
    let title = if win == "ball" { "悬浮球" } else { "触屏键盘" };
    let mut xid = state.xids.lock().ok().and_then(|m| m.get(win).copied());
    if xid.is_none() {
        xid = with_xconn(app, |x| x.find_window_id(title));
        if let Some(id) = xid {
            if let Ok(mut m) = state.xids.lock() {
                m.insert(win.to_string(), id);
            }
        }
    }
    let (Some(xid), Some(rects)) = (xid, rects) else {
        eprintln!("[shape] {win}: xid或区域缺失");
        return None;
    };
    let click_through = win == "ball" || state.click_through.load(Ordering::Relaxed);
    let sf = app
        .get_webview_window(win)
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let r = with_xconn(app, |x| {
        if click_through {
            x.set_input_shape(xid, &rects, sf).ok()
        } else {
            x.clear_input_shape(xid).ok()
        }
    });
    eprintln!("[shape] {win}: xid={xid} rects={} sf={sf} ct={click_through} result={r:?}", rects.len());
    r
}

fn monitor_of(app: &AppHandle) -> Option<tauri::Monitor> {
    let win = app.get_webview_window("keyboard")?;
    win.current_monitor()
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten())
}

/// 读取 X11 工作区 (扣除 dock/面板后的可用区域, 全局物理坐标)。
/// 可用 TOUCH_KB_WORKAREA=x,y,w,h 环境变量手动指定 (用于 WM 未维护工作区时的 dock 避让)。
fn workarea_physical(app: &AppHandle) -> Option<(i32, i32, u32, u32)> {
    if let Ok(s) = std::env::var("TOUCH_KB_WORKAREA") {
        let nums: Vec<i32> = s
            .split(',')
            .filter_map(|p| p.trim().parse::<i32>().ok())
            .collect();
        if nums.len() == 4 {
            return Some((nums[0], nums[1], nums[2].max(0) as u32, nums[3].max(0) as u32));
        }
    }
    let state = app.state::<AppState>();
    let mut guard = state.xconn.lock().ok()?;
    if guard.is_none() {
        *guard = xutil::XConn::new().ok();
    }
    guard.as_ref()?.workarea()
}

/// X11 下让窗口不接受焦点 (点按键/悬浮球时不抢目标应用焦点)。
/// 通过 GTK 设置, 保证 WM_HINTS 由 GTK 维护、不被覆盖。
/// 注意: tao 在窗口以 focus=false 创建后, 会在首次绘制时把 accept_focus 改回 true
/// (见 tao window.rs 的 connect_draw 恢复逻辑), 因此必须在每次绘制后重新关闭。
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn disable_focus(win: &WebviewWindow) {
    if let Ok(gw) = win.gtk_window() {
        use gtk::prelude::*;
        gw.set_accept_focus(false);
        gw.set_focus_on_map(false);
        gw.connect_draw(move |w, _ctx| {
            w.set_accept_focus(false);
            gtk::glib::Propagation::Proceed
        });
    }
}

/// 键盘窗口逻辑高度: 完整布局 = 宽度 × 高宽比 (键帽近正方, 参照 Windows 屏幕键盘);
/// 分裂布局 = 键距 × (行数×0.9 + 顶栏/边距)。均乘用户缩放, 并夹取进屏幕高度。
/// 注意: 分裂宽度设置 (split_half_units) 只改两半面板的宽度 (= 键距 × 键距数),
/// 键距本身始终按默认 6.8 推算, 因此窗口高度与该设置无关。
fn kb_logical_height(w: f64, lh: f64, s: &Settings) -> f64 {
    let base = if s.mode == "split" {
        let half_w = (w * 0.39).min(520.0);
        let pitch = half_w / SPLIT_HALF_UNITS;
        let rows = if s.num_row { 6.0 } else { 5.0 };
        pitch * (rows * 0.9 + 1.35) // 键区 (每行 0.9 键距) + 半面板自带标题栏/内边距
    } else {
        KB_ASPECT * w
    };
    (base * s.kb_scale).clamp(170.0, lh * 0.85)
}

/// 悬浮模式逻辑尺寸: 宽 = 工作区宽 × float_width, 高按同样的高宽比推算
fn float_logical_size(lw: f64, lh: f64, s: &Settings) -> (f64, f64) {
    let wr = if (0.2..=1.0).contains(&s.float_width) { s.float_width } else { 0.6 };
    let fw = (lw * wr).clamp(280.0, lw);
    let fh = kb_logical_height(fw, lh, s);
    (fw, fh)
}

/// 按屏幕尺寸、工作区与设置调整键盘窗口几何 (自动适配屏幕)。
/// 停靠模式贴工作区底部整行; 悬浮模式使用记录位置 (无记录时以当前窗口顶边中点为锚分离)。
fn refit(app: &AppHandle) {
    let Some(win) = app.get_webview_window("keyboard") else { return };
    let Some(m) = monitor_of(app) else { return };
    let sf = m.scale_factor();

    // 显示器矩形 (物理像素), 再扣除 dock/面板占据的工作区
    let mut mx = m.position().x as f64;
    let mut my = m.position().y as f64;
    let mut mw = m.size().width as f64;
    let mut mh = m.size().height as f64;
    if let Some((ax, ay, aw, ah)) = workarea_physical(app) {
        let l = mx.max(ax as f64);
        let t = my.max(ay as f64);
        let r = (mx + mw).min(ax as f64 + aw as f64);
        let b = (my + mh).min(ay as f64 + ah as f64);
        if r > l && b > t {
            mx = l;
            my = t;
            mw = r - l;
            mh = b - t;
        }
    }

    let lw = mw / sf;
    let lh = mh / sf;
    let state = app.state::<AppState>();
    let s = state.settings.lock().unwrap().clone();
    if s.float_mode {
        let (fw, fh) = float_logical_size(lw, lh, &s);
        let fwpx = (fw * sf).round();
        let fhpx = (fh * sf).round();
        let mut pos = match s.kb_pos {
            Some([x, y]) => {
                let mut px = x as f64;
                // 悬浮宽度变化时保持窗口中心 X 不动: 设置面板水平居中于窗口,
                // 中心不动则「悬浮宽度」滑轨不在指针下漂移,
                // 否则调节宽度时取值来回跳, 窗口忽大忽小 (已踩坑)。
                // 以上次 refit 应用的宽度为基准 —— GTK 尺寸异步生效, outer_size 可能滞后
                let last_w = *state.kb_float_w.lock().unwrap();
                if last_w > 0.0 {
                    px -= (fwpx - last_w) / 2.0;
                }
                (px, y as f64)
            }
            // 首次进入悬浮: 保持当前窗口顶边中点不动, 视觉上从停靠位置"分离"
            None => match (win.outer_position(), win.outer_size()) {
                (Ok(p), Ok(sz)) => {
                    (p.x as f64 + sz.width as f64 / 2.0 - fwpx / 2.0, p.y as f64)
                }
                _ => (mx + (mw - fwpx) / 2.0, my + mh - fhpx),
            },
        };
        // 夹取进工作区, 防止拖到屏幕外 / 叠进 dock
        pos.0 = pos.0.clamp(mx, (mx + mw - fwpx).max(mx));
        pos.1 = pos.1.clamp(my, (my + mh - fhpx).max(my));
        // 把解析后的实际位置写回 kb_pos (随 settings-changed 回流前端持久化),
        // 并记录本次宽度作为下次宽度变化的中心锚 —— 否则 kb_pos 与窗口实际位置脱节,
        // 连续调整宽度时会从过期位置锚定导致漂移 (已踩坑)
        if let Ok(mut st) = state.settings.lock() {
            st.kb_pos = Some([pos.0.round() as i32, pos.1.round() as i32]);
        }
        if let Ok(mut w) = state.kb_float_w.lock() {
            *w = fwpx;
        }
        let _ = win.set_size(LogicalSize::new(fw, fh));
        let _ = win.set_position(PhysicalPosition::new(pos.0.round() as i32, pos.1.round() as i32));
    } else {
        let h = kb_logical_height(lw, lh, &s);
        let _ = win.set_size(LogicalSize::new(lw, h));
        let px = mx.round() as i32;
        let py = (my + mh) as i32 - (h * sf).round() as i32;
        let _ = win.set_position(PhysicalPosition::new(px, py));
    }
    let _ = win.emit(
        "layout-changed",
        ScreenInfo {
            logical_width: lw,
            logical_height: lh,
            scale_factor: sf,
            workarea: workarea_physical(app)
                .map(|(x, y, w, h)| [x as f64 / sf, y as f64 / sf, w as f64 / sf, h as f64 / sf]),
        },
    );
}

/// 悬浮球默认落点: 工作区右缘 (避让 dock)。
/// 返回的是窗口位置: GTK 强制球窗口最小 200x200, 贴右缘时窗口右缘留 margin、
/// 球体锚定窗口右上角 (配合前端 c-h-r 类), 整窗留在屏幕内不被 WM 拉回。
fn default_ball_pos(app: &AppHandle) -> (i32, i32) {
    let Some(m) = monitor_of(app) else { return (900, 300) };
    let sf = m.scale_factor();
    let margin = (8.0 * sf).round() as i32;
    let mut l = m.position().x;
    let mut r = m.position().x + m.size().width as i32;
    let mut y = m.position().y + (m.size().height as f64 * 0.35) as i32;
    if let Some((ax, ay, aw, ah)) = workarea_physical(app) {
        l = l.max(ax);
        r = r.min(ax + aw as i32);
        y = y.clamp(ay + margin, ay + ah as i32 - margin);
    }
    let wwin = fallback_ball_win_size(sf);
    (r - margin - wwin, y)
}

/// 球窗口的假定物理宽度 (GTK 最小 200 逻辑像素; 窗口未 map 时 outer_size 不可靠)
fn fallback_ball_win_size(sf: f64) -> i32 {
    (200.0 * sf).round() as i32
}

fn hide_kb_show_ball(app: &AppHandle) {
    if let Some(kb) = app.get_webview_window("keyboard") {
        let _ = kb.hide();
    }
    // 设置窗口独立于键盘窗口, 键盘收起时一并隐藏
    if let Some(sw) = app.get_webview_window("settings") {
        let _ = sw.hide();
    }
    if let Some(ball) = app.get_webview_window("ball") {
        // GTK 可能忽略早期尺寸设置, 显示时再设一次
        let _ = ball.set_size(LogicalSize::new(BALL_SIZE, BALL_SIZE));
        let pos = match *app.state::<AppState>().ball_pos.lock().unwrap() {
            Some(p) => p,
            None => {
                let p = default_ball_pos(app);
                *app.state::<AppState>().ball_pos.lock().unwrap() = Some(p);
                p
            }
        };
        let _ = ball.set_position(PhysicalPosition::new(pos.0, pos.1));
        let _ = ball.show();
        // 通知前端球体锚定角 (贴右/下边缘时锚定窗口右/下角)
        let (h_right, v_bottom) = *app.state::<AppState>().ball_corner.lock().unwrap();
        let _ = ball.emit("ball-corner", (h_right, v_bottom));
        // 首次显示时球的 X 窗口才创建, 显示后立即应用输入区域 (球体外穿透)
        let _ = apply_input_shape(app, "ball");
    }
}

fn show_kb_hide_ball(app: &AppHandle) {
    if let Some(ball) = app.get_webview_window("ball") {
        let _ = ball.hide();
    }
    if let Some(kb) = app.get_webview_window("keyboard") {
        refit(app);
        let _ = kb.show();
    }
}

fn toggle_keyboard(app: &AppHandle) {
    let visible = app
        .get_webview_window("keyboard")
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    if visible {
        hide_kb_show_ball(app);
    } else {
        show_kb_hide_ball(app);
    }
}

fn do_apply_settings(app: &AppHandle, s: Settings) {
    *app.state::<AppState>().settings.lock().unwrap() = s.clone();
    refit(app);
    // refit 可能为悬浮模式补全 kb_pos, 回显解析后的完整设置供前端持久化。
    // 键盘与设置窗口都订阅此事件, 两边状态保持一致
    let resolved = app.state::<AppState>().settings.lock().unwrap().clone();
    for label in ["keyboard", "settings", "touchpad"] {
        if let Some(win) = app.get_webview_window(label) {
            let _ = win.emit("settings-changed", &resolved);
        }
    }
}

// ---------- Tauri 命令 ----------

#[tauri::command]
fn get_screen_info(app: AppHandle) -> Result<ScreenInfo, String> {
    let m = monitor_of(&app).ok_or("无法获取显示器信息")?;
    let sf = m.scale_factor();
    Ok(ScreenInfo {
        logical_width: m.size().width as f64 / sf,
        logical_height: m.size().height as f64 / sf,
        scale_factor: sf,
        workarea: workarea_physical(&app)
            .map(|(x, y, w, h)| [x as f64 / sf, y as f64 / sf, w as f64 / sf, h as f64 / sf]),
    })
}

#[tauri::command]
fn update_hit_regions(app: AppHandle, state: State<AppState>, win: String, rects: Vec<f64>) {
    let mut out = Vec::with_capacity(rects.len() / 4);
    for c in rects.chunks(4) {
        if c.len() == 4 {
            out.push([c[0], c[1], c[2], c[3]]);
        }
    }
    eprintln!("[shape] update_hit_regions {win}: {} rects", out.len());
    state.hit_regions.lock().unwrap().insert(win.clone(), out);
    let _ = apply_input_shape(&app, &win);
}

#[tauri::command]
fn set_click_through(app: AppHandle, enabled: bool) {
    app.state::<AppState>().click_through.store(enabled, Ordering::Relaxed);
    let _ = apply_input_shape(&app, "keyboard");
}

#[tauri::command]
async fn tap_key(
    state: State<'_, AppState>,
    code: String,
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
) -> Result<(), String> {
    let input = state.input.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = input.lock().map_err(|_| "内部锁错误".to_string())?;
        match guard.as_ref() {
            Some(b) => b.tap(&code, shift, ctrl, alt, meta),
            None => Err("输入后端不可用: 请按 README 配置 uinput 权限或使用 X11 会话".into()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn input_backend_name(state: State<AppState>) -> String {
    state
        .input
        .lock()
        .unwrap()
        .as_ref()
        .map(|b| b.name().to_string())
        .unwrap_or_else(|| "none".into())
}

#[tauri::command]
fn get_ime_label(state: State<AppState>) -> String {
    state.ime_label.lock().unwrap().clone()
}

#[tauri::command]
fn apply_settings(app: AppHandle, settings: Settings) {
    do_apply_settings(&app, settings);
}

#[tauri::command]
fn minimize_to_ball(app: AppHandle) {
    hide_kb_show_ball(&app);
}

#[tauri::command]
fn restore_keyboard(app: AppHandle) {
    show_kb_hide_ball(&app);
}

#[tauri::command]
fn ball_moved(state: State<AppState>, x: i32, y: i32) {
    *state.ball_pos.lock().unwrap() = Some((x, y));
}

/// 悬浮球拖动开始 (JS pointerdown)。拖动跟随由 Rust 线程用全局光标坐标完成,
/// 避免窗口内坐标在窗口移动后失真 (闪烁/拖不动的根因)。
#[tauri::command]
fn start_ball_drag(state: State<AppState>) {
    eprintln!("[ball-drag] start");
    state.ball_dragging.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn end_ball_drag(app: AppHandle, state: State<AppState>) {
    eprintln!("[ball-drag] end (active={})", state.ball_drag_active.load(Ordering::Relaxed));
    state.ball_dragging.store(false, Ordering::Relaxed);
    // 快速点按时拖动线程可能尚未进入跟随状态, 直接按点击处理
    if !state.ball_drag_active.load(Ordering::Relaxed) {
        show_kb_hide_ball(&app);
    }
}

#[tauri::command]
fn ball_input_mode(state: State<AppState>) -> String {
    if state.cursor_tracking.load(Ordering::Relaxed) {
        "rust".into()
    } else {
        "js".into()
    }
}

/// 键盘拖动开始 (JS 在拖动把手上 pointerdown/touchstart)。
/// 跟随由 Rust 线程用全局光标坐标完成, 避免窗口内坐标在窗口移动后失真。
#[tauri::command]
fn start_kb_drag(state: State<AppState>) {
    state.kb_dragging.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn end_kb_drag(state: State<AppState>) {
    state.kb_dragging.store(false, Ordering::Relaxed);
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ---------- 设置窗口 (独立窗体, 不与键盘共用) ----------

/// 设置窗口显隐: 已可见则隐藏, 否则定位到键盘上方并显示。
/// 窗口与键盘一样不抢焦点 (disable_focus + WM_HINTS input=False),
/// 调整设置时目标应用的焦点保持不变, 键盘仍可正常打字。
#[tauri::command]
fn toggle_settings(app: AppHandle) {
    let Some(sw) = app.get_webview_window("settings") else { return };
    eprintln!("[settings] toggle: visible={:?}", sw.is_visible());
    if sw.is_visible().unwrap_or(false) {
        let _ = sw.hide();
        return;
    }
    position_settings(&app, &sw);
    let _ = sw.show();
    // 首次显示前 GTK 可能还没完成尺寸分配 (outer_size 返回 0), 显示后再精确定位一次
    position_settings(&app, &sw);
    // X11: 设置窗口同样设为不抢焦点 (窗口可能启动后才 map, 每次显示时补一次)
    with_xconn(&app, |x| x.relax_focus(&["键盘设置"]).ok());
}

#[tauri::command]
fn hide_settings(app: AppHandle) {
    if let Some(sw) = app.get_webview_window("settings") {
        let _ = sw.hide();
    }
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

/// 设置窗口定位: 水平居中于键盘窗口, 底边贴键盘顶边上方; 键盘不可见时居中于工作区。
/// 全程夹取进工作区, 不被 dock 遮挡。
fn position_settings(app: &AppHandle, sw: &WebviewWindow) {
    let Some(m) = monitor_of(app) else { return };
    let sf = m.scale_factor();
    let (wx, wy, ww, wh) = workarea_physical(app)
        .map(|(x, y, w, h)| (x as f64, y as f64, w as f64, h as f64))
        .unwrap_or((
            m.position().x as f64,
            m.position().y as f64,
            m.size().width as f64,
            m.size().height as f64,
        ));
    let (sww, swh) = sw
        .outer_size()
        .ok()
        .map(|s| (s.width as f64, s.height as f64))
        .filter(|(w, h)| *w > 0.0 && *h > 0.0) // 未显示时 GTK 可能还没分配尺寸
        .unwrap_or((400.0 * sf, 700.0 * sf));
    let kb_geom = app.get_webview_window("keyboard").and_then(|kb| {
        let p = kb.outer_position().ok()?;
        let s = kb.outer_size().ok()?;
        Some((kb.is_visible().unwrap_or(false), p.x as f64, p.y as f64, s.width as f64))
    });
    let (cx, top) = match kb_geom {
        Some((true, kx, ky, kw)) => (kx + kw / 2.0, ky - swh - 12.0 * sf),
        _ => (wx + ww / 2.0, wy + (wh - swh) / 2.0),
    };
    let x = (cx - sww / 2.0).clamp(wx, (wx + ww - sww).max(wx));
    let y = top.clamp(wy + 8.0 * sf, (wy + wh - swh).max(wy + 8.0 * sf));
    let _ = sw.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

// ---------- 触摸板窗口 (屏幕触摸板, 类 Windows 系统触摸板) ----------

/// 触摸板窗口显隐切换 (托盘菜单 / 窗口 ✕ / 调试启动 TOUCH_KB_TP_SHOW=1)
#[tauri::command]
fn toggle_touchpad(app: AppHandle) {
    let Some(tp) = app.get_webview_window("touchpad") else { return };
    if tp.is_visible().unwrap_or(false) {
        hide_touchpad(&app);
    } else {
        show_touchpad(&app);
    }
}

fn show_touchpad(app: &AppHandle) {
    let Some(tp) = app.get_webview_window("touchpad") else { return };
    position_touchpad(app, &tp);
    let _ = tp.show();
    // 首次显示前 GTK 可能未完成尺寸分配 (outer_size 返回 0), 显示后再精确定位一次
    position_touchpad(app, &tp);
    // X11: 不抢焦点 (窗口可能启动后才 map, 每次显示时补一次)
    with_xconn(app, |x| x.relax_focus(&["触摸板"]).ok());
    update_tray_tp(app, true);
}

fn hide_touchpad(app: &AppHandle) {
    if let Some(tp) = app.get_webview_window("touchpad") {
        let _ = tp.hide();
    }
    // Moved 事件已把最新位置写进 settings.tp_pos, 回显给前端持久化
    let s = app.state::<AppState>().settings.lock().unwrap().clone();
    do_apply_settings(app, s);
    update_tray_tp(app, false);
}

/// 托盘「触摸板」勾选项跟随窗口显隐
fn update_tray_tp(app: &AppHandle, visible: bool) {
    if let Ok(item) = app.state::<AppState>().tray_tp.lock() {
        if let Some(it) = item.as_ref() {
            let _ = it.set_checked(visible);
        }
    }
}

/// 触摸板窗口定位: 有记录位置则沿用 (夹取进工作区, 换屏幕/改分辨率后不跑丢);
/// 否则键盘可见时水平居中于键盘、底边贴键盘顶上方, 键盘收起时居中于工作区。
fn position_touchpad(app: &AppHandle, tp: &WebviewWindow) {
    let Some(m) = monitor_of(app) else { return };
    let sf = m.scale_factor();
    let (wx, wy, ww, wh) = workarea_physical(app)
        .map(|(x, y, w, h)| (x as f64, y as f64, w as f64, h as f64))
        .unwrap_or((
            m.position().x as f64,
            m.position().y as f64,
            m.size().width as f64,
            m.size().height as f64,
        ));
    let (tw, th) = tp
        .outer_size()
        .ok()
        .map(|s| (s.width as f64, s.height as f64))
        .filter(|(w, h)| *w > 0.0 && *h > 0.0)
        .unwrap_or((TP_WIN_W * sf, TP_WIN_H * sf));
    let s = app.state::<AppState>().settings.lock().unwrap().clone();
    let (x, y) = match s.tp_pos {
        Some([x, y]) => (x as f64, y as f64),
        None => {
            let kb_geom = app.get_webview_window("keyboard").and_then(|kb| {
                let p = kb.outer_position().ok()?;
                let sz = kb.outer_size().ok()?;
                Some((kb.is_visible().unwrap_or(false), p.x as f64, p.y as f64, sz.width as f64))
            });
            match kb_geom {
                Some((true, kx, ky, kw)) => (kx + kw / 2.0 - tw / 2.0, ky - th - 12.0 * sf),
                _ => (wx + (ww - tw) / 2.0, wy + (wh - th) / 2.0),
            }
        }
    };
    let x = x.clamp(wx, (wx + ww - tw).max(wx));
    let y = y.clamp(wy + 8.0 * sf, (wy + wh - th).max(wy + 8.0 * sf));
    let _ = tp.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

// ---------- 触摸板鼠标注入 ----------
// mutter 的触摸指针模拟会在触摸期间把合成器光标"胶"在手指位置 (绝对吸附),
// 与注入的相对移动互相拉锯 = 光标闪烁。因此:
// - 手势期间不注入相对移动; 手指位移读自被"胶"住的光标 (它就是手指位置的镜像),
//   维护虚拟指针 V: V += 手指位移 × 增益, 用画笔绝对跳变持续把光标设为 V;
//   被吸回手指的"鬼影"发生在触摸板窗口内 —— 窗口光标为透明 (CSS cursor:none),
//   鬼影不可见, 用户只看到 V。
// - 点击/拖动在抬手或长按触发时先跳到 V, 等合成器应用 (~35ms) 再按键。
// - 拖动 = 表面长按 ~350ms 后滑动, 松手结束 (单指完成, 避开指针模拟只跟一指的问题)。

async fn with_mouse(
    state: &State<'_, AppState>,
    f: impl FnOnce(&(dyn MouseBackend + Send)) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let mouse = state.mouse.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = mouse.lock().map_err(|_| "内部锁错误".to_string())?;
        match guard.as_ref() {
            Some(b) => f(b.as_ref()),
            None => Err("鼠标后端不可用: 触摸板需要 /dev/uinput 权限或 X11 会话".into()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 手势开始: 刷新增益 (工作区宽/板宽 × 速度) 并初始化 V (无记录时取当前光标)
fn tp_gesture_begin(app: &AppHandle) {
    let state = app.state::<AppState>();
    let s = state.settings.lock().unwrap().clone();
    let w = workarea_physical(app).map(|(_, _, w, _)| w as f64).unwrap_or(1440.0);
    let padw = app
        .get_webview_window("touchpad")
        .and_then(|t| t.outer_size().ok())
        .map(|sz| sz.width as f64)
        .filter(|w2| *w2 > 0.0)
        .unwrap_or(TP_WIN_W);
    *state.tp_gain.lock().unwrap() = (w / padw).clamp(1.2, 6.0) * s.tp_speed.clamp(0.3, 3.0);
    let mut v = state.tp_vpos.lock().unwrap();
    if v.is_none() {
        if let Some(c) = with_xconn(app, |x| x.cursor_pos().ok()) {
            *v = Some(c);
        }
    }
}

/// 单步伺服: 把光标向 V 推进一步 (阻尼 damp), 已到位返回 true。
/// XTEST 后端直接绝对定位; uinput 无绝对定位 (mutter 不转发 XTEST 移动、
/// 忽略虚拟画笔, 已实测), 用阻尼相对移动几何收敛 —— 反馈环路无稳态误差,
/// 即使有加速度失配也收敛。
fn tp_step_toward_v(app: &AppHandle, damp: f64) -> bool {
    let Some((cx, cy)) = with_xconn(app, |x| x.cursor_pos().ok()) else {
        return false;
    };
    let state = app.state::<AppState>();
    let Some((vx, vy)) = *state.tp_vpos.lock().unwrap() else {
        return false;
    };
    let (dx, dy) = (vx - cx, vy - cy);
    if dx.abs() + dy.abs() < 2.0 {
        return true;
    }
    let moved = match state.mouse.lock() {
        Ok(g) => match g.as_ref() {
            Some(b) => {
                if b.warp(vx as i64, vy as i64, 0, 0).is_err() {
                    let _ = b.move_rel(dx * damp, dy * damp);
                }
                true
            }
            None => false,
        },
        Err(_) => false,
    };
    moved
}

/// 伺服收敛: 连续推进直到到位 (点击/拖动按下前用, ~几十 ms)
fn tp_servo_v(app: &AppHandle) -> bool {
    for _ in 0..14 {
        if tp_step_toward_v(app, 0.5) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(9));
    }
    false
}

/// 表面触点数变化 (JS touchstart/end 上报)。0→≥1 手势开始。
/// 手势结束 (→0) 后由手势线程的"回位窗口"伺服把光标拉回 V (鬼影位置在板上,
/// 伺服把它带回 V 后恢复可见)。
#[tauri::command]
fn tp_surface(app: AppHandle, state: State<AppState>, fingers: u8) {
    let prev = state.tp_fingers.swap(fingers.min(5), Ordering::Relaxed);
    if prev == 0 && fingers > 0 {
        state.tp_js_motion.store(false, Ordering::Relaxed);
        state.tp_travel.store(0, Ordering::Relaxed);
        tp_gesture_begin(&app);
    }
}

/// 手指位移 (物理像素, 未乘增益; WebKitGTK 派发 touchmove 时由前端上报)。
/// 只更新 V; 光标由手势线程的伺服追赶 (touchmove 模式实时追, 抬手后回位)。
#[tauri::command]
fn tp_move(state: State<AppState>, dx: f64, dy: f64) {
    static DBG: OnceLock<bool> = OnceLock::new();
    if *DBG.get_or_init(|| std::env::var("TOUCH_KB_DEBUG").is_ok()) {
        eprintln!("[tp] move dx={dx:.1} dy={dy:.1}");
    }
    state.tp_js_motion.store(true, Ordering::Relaxed);
    let gain = *state.tp_gain.lock().unwrap();
    {
        let mut v = state.tp_vpos.lock().unwrap();
        if let Some((vx, vy)) = *v {
            *v = Some((vx + dx * gain, vy + dy * gain));
        }
    }
    state.tp_travel.fetch_add((dx.abs() + dy.abs()) as u32, Ordering::Relaxed);
}

/// 完整点击 (轻触/左右按键): 行程判定 (≤20px 为轻触) → 伺服到 V → 按键
#[tauri::command]
async fn tp_click(app: AppHandle, state: State<'_, AppState>, btn: u8) -> Result<(), String> {
    let travel = state.tp_travel.load(Ordering::Relaxed);
    let ready = travel <= 20 && tp_servo_v(&app);
    with_mouse(&state, move |b| {
        if !ready {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
        b.button(btn, true)?;
        std::thread::sleep(Duration::from_millis(18));
        b.button(btn, false)
    })
    .await
}

/// 长按拖动: 表面长按 ~350ms 触发 press (伺服到 V 后按下左键), 松手 release
#[tauri::command]
async fn tp_drag(app: AppHandle, state: State<'_, AppState>, press: bool) -> Result<(), String> {
    let ok = !press || tp_servo_v(&app);
    with_mouse(&state, move |b| {
        if !ok {
            return Ok(());
        }
        if press {
            std::thread::sleep(Duration::from_millis(20));
        }
        b.button(1, press)
    })
    .await
}

/// 滚轮 (整数格数: dy 正=向下, dx 正=向右); 滚轮与光标位置无关, 不受胶合影响
#[tauri::command]
async fn tp_scroll(state: State<'_, AppState>, dx: i32, dy: i32) -> Result<(), String> {
    static DBG: OnceLock<bool> = OnceLock::new();
    if *DBG.get_or_init(|| std::env::var("TOUCH_KB_DEBUG").is_ok()) {
        eprintln!("[tp] scroll dx={dx} dy={dy}");
    }
    with_mouse(&state, move |b| b.scroll(dx, dy)).await
}

#[tauri::command]
fn mouse_backend_name(state: State<AppState>) -> String {
    state
        .mouse
        .lock()
        .unwrap()
        .as_ref()
        .map(|b| b.name().to_string())
        .unwrap_or_else(|| "none".into())
}


// ---------- 系统深浅色 (主题 = 跟随系统) ----------

/// GNOME color-scheme 是否 prefer-dark; 非 GNOME/查询失败按浅色处理
fn prefers_dark() -> bool {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("prefer-dark"))
        .unwrap_or(false)
}

#[tauri::command]
fn get_prefers_dark() -> bool {
    prefers_dark()
}

/// 系统深浅色轮询线程: 变化时 emit `prefers-dark-changed`,
/// 主题设置为「跟随系统」的两个前端窗口即时切换
fn theme_poll(app: AppHandle) {
    let mut last = prefers_dark();
    loop {
        std::thread::sleep(Duration::from_secs(3));
        let cur = prefers_dark();
        if cur != last {
            last = cur;
            eprintln!("[theme] 系统深色 -> {cur}");
            let _ = app.emit("prefers-dark-changed", cur);
        }
    }
}

// ---------- 「开始」键图标: 系统 start-here 图标, 企鹅为前端兜底 ----------

/// base64 编码 (标准字母表 + 填充), 用于把图标文件转 data URL
fn b64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = ((c[0] as u32) << 16) | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn icon_mime(ext: &str) -> Option<&'static str> {
    match ext {
        "svg" => Some("image/svg+xml"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None, // xpm 等浏览器渲染不了
    }
}

fn read_icon_data_url(p: &Path) -> Option<String> {
    let ext = p.extension()?.to_str()?.to_lowercase();
    let mime = icon_mime(&ext)?;
    let data = std::fs::read(p).ok()?;
    Some(format!("data:{mime};base64,{}", b64_encode(&data)))
}

/// 图标主题目录 ($HOME/.icons, $XDG_DATA_DIRS/icons, /usr/share/pixmaps)
fn icon_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".icons"));
        dirs.push(PathBuf::from(&home).join(".local/share/icons"));
    }
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(d).join("icons"));
    }
    dirs.push(PathBuf::from("/usr/share/pixmaps"));
    dirs
}

/// 当前图标主题 (gsettings), 回退 hicolor
fn current_icon_theme() -> String {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "hicolor".into())
}

/// 主题继承链 (各主题 index.theme 的 Inherits), 末尾必有 hicolor
fn theme_chain(theme: &str) -> Vec<String> {
    let mut chain = vec![theme.to_string()];
    let mut i = 0;
    while i < chain.len() && chain.len() < 6 {
        let idx = icon_theme_dirs().into_iter().find_map(|base| {
            std::fs::read_to_string(base.join(&chain[i]).join("index.theme")).ok()
        });
        if let Some(txt) = idx {
            if let Some(line) = txt.lines().find(|l| l.trim_start().starts_with("Inherits")) {
                if let Some(v) = line.split('=').nth(1) {
                    for inh in v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                        if !chain.contains(&inh) {
                            chain.push(inh);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    if !chain.iter().any(|t| t == "hicolor") {
        chain.push("hicolor".into());
    }
    chain
}

/// 在目录里递归 (深度 4) 搜索名字在 stems 中的图标文件。
/// 得分: svg 优先于位图, 同类中目录名前导数字 (尺寸) 越大越好。
fn search_icon_dir(dir: &Path, stems: &[&str]) -> Option<PathBuf> {
    fn walk(dir: &Path, stems: &[&str], depth: usize, best: &mut Option<(i32, PathBuf)>) {
        if depth > 4 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, stems, depth + 1, best);
                continue;
            }
            let (Some(stem), Some(ext)) = (
                p.file_stem().and_then(|s| s.to_str()),
                p.extension().and_then(|s| s.to_str()),
            ) else {
                continue;
            };
            let Some(rank) = stems.iter().position(|s| *s == stem) else { continue };
            let mime_score = match ext.to_lowercase().as_str() {
                "svg" => 10000,
                "png" | "gif" | "jpg" | "jpeg" | "webp" => 0,
                _ => continue,
            };
            let size_score = p
                .parent()
                .and_then(|d| d.file_name().and_then(|n| n.to_str()))
                .and_then(|n| {
                    n.split(['x', 'X', '@'])
                        .next()?
                        .parse::<i32>()
                        .ok()
                })
                .unwrap_or(0);
            let score = mime_score + size_score - rank as i32; // 同尺寸下 start-here 优先
            if best.as_ref().is_none_or(|(bs, _)| score > *bs) {
                *best = Some((score, p.clone()));
            }
        }
    }
    let mut best = None;
    walk(dir, stems, 0, &mut best);
    best.map(|(_, p)| p)
}

/// 查找「开始」键的系统图标 (freedesktop 规范名 start-here / distributor-logo),
/// 返回 data URL; 找不到返回 None (前端用内置 Linux 企鹅兜底)。结果缓存。
fn find_start_icon() -> Option<String> {
    let stems = ["start-here", "distributor-logo"];
    let bases = icon_theme_dirs();
    // /usr/share/pixmaps 是无主题结构的平铺目录, 直接搜一遍
    for b in &bases {
        if b.ends_with("pixmaps") {
            if let Some(p) = search_icon_dir(b, &stems) {
                if let Some(u) = read_icon_data_url(&p) {
                    return Some(u);
                }
            }
        }
    }
    for theme in theme_chain(&current_icon_theme()) {
        for b in &bases {
            if b.ends_with("pixmaps") {
                continue;
            }
            if let Some(p) = search_icon_dir(&b.join(&theme), &stems) {
                if let Some(u) = read_icon_data_url(&p) {
                    return Some(u);
                }
            }
        }
    }
    None
}

static START_ICON: OnceLock<Option<String>> = OnceLock::new();

#[tauri::command]
fn start_icon_url() -> Option<String> {
    START_ICON.get_or_init(find_start_icon).clone()
}


// ---------- 后台线程 ----------

/// 悬浮球拖动跟随: 用 X 全局光标坐标 (鼠标与触摸均由 XWayland 同步到核心指针),
/// 窗口位置 = 起始窗口位置 + (当前光标 - 起始光标), 与窗口自身移动无关, 不闪不抖。
fn ball_drag_poll(app: AppHandle) {
    let Ok(xconn) = xutil::XConn::new() else { return };
    let mut active = false;
    let mut start_g = (0.0f64, 0.0f64);
    let mut start_w = (0i32, 0i32);
    let mut moved = false;
    loop {
        std::thread::sleep(Duration::from_millis(16));
        let flag = app.state::<AppState>().ball_dragging.load(Ordering::Relaxed);
        if flag && !active {
            let Some(ball) = app.get_webview_window("ball") else { continue };
            if let (Ok(g), Ok(w)) = (xconn.cursor_pos(), ball.outer_position()) {
                start_g = g;
                start_w = (w.x, w.y);
                moved = false;
                active = true;
                app.state::<AppState>().ball_drag_active.store(true, Ordering::Relaxed);
            }
        } else if flag && active {
            let Some(ball) = app.get_webview_window("ball") else { continue };
            if let Ok((gx, gy)) = xconn.cursor_pos() {
                let dx = gx - start_g.0;
                let dy = gy - start_g.1;
                if dx.abs() + dy.abs() > 6.0 {
                    moved = true;
                }
                let _ = ball.set_position(PhysicalPosition::new(
                    start_w.0 + dx.round() as i32,
                    start_w.1 + dy.round() as i32,
                ));
            }
        } else if !flag && active {
            active = false;
            app.state::<AppState>().ball_drag_active.store(false, Ordering::Relaxed);
            // X11/GTK 非线程安全: 落点计算 (current_monitor 等) 必须回主线程
            let app2 = app.clone();
            let app3 = app.clone();
            let _ = app.run_on_main_thread(move || {
                if moved {
                    snap_ball(&app2);
                } else {
                    show_kb_hide_ball(&app3);
                }
            });
        }
    }
}

/// 键盘拖动跟随: 与悬浮球同机制, 用 X 全局光标坐标 (触摸由 XWayland 同步到核心指针),
/// 窗口位置 = 起始窗口位置 + (当前光标 - 起始光标), 与窗口自身移动无关, 不闪不抖。
fn kb_drag_poll(app: AppHandle) {
    let Ok(xconn) = xutil::XConn::new() else { return };
    let mut active = false;
    let mut start_g = (0.0f64, 0.0f64);
    let mut start_w = (0i32, 0i32);
    let mut moved = false;
    loop {
        std::thread::sleep(Duration::from_millis(16));
        let flag = app.state::<AppState>().kb_dragging.load(Ordering::Relaxed);
        if flag && !active {
            let Some(kb) = app.get_webview_window("keyboard") else { continue };
            if let (Ok(g), Ok(w)) = (xconn.cursor_pos(), kb.outer_position()) {
                start_g = g;
                start_w = (w.x, w.y);
                moved = false;
                active = true;
                app.state::<AppState>().kb_drag_active.store(true, Ordering::Relaxed);
            }
        } else if flag && active {
            let Some(kb) = app.get_webview_window("keyboard") else { continue };
            if let Ok((gx, gy)) = xconn.cursor_pos() {
                let dx = gx - start_g.0;
                let dy = gy - start_g.1;
                if dx.abs() + dy.abs() > 6.0 {
                    moved = true;
                }
                let _ = kb.set_position(PhysicalPosition::new(
                    start_w.0 + dx.round() as i32,
                    start_w.1 + dy.round() as i32,
                ));
            }
        } else if !flag && active {
            active = false;
            app.state::<AppState>().kb_drag_active.store(false, Ordering::Relaxed);
            // 拖动结束的几何处理 (refit/current_monitor) 必须回主线程: GTK 的 X 连接非线程安全
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || finish_kb_drag(&app2, moved));
        }
    }
}

/// 键盘拖动结束 (主线程): 拖动过则转为悬浮模式。kb_pos 留空,
/// refit 会以拖动落点的窗口顶边中点为锚缩小窗口并夹取进工作区, 随后回显给前端持久化。
fn finish_kb_drag(app: &AppHandle, moved: bool) {
    if !moved {
        return; // 把手上的轻点不算拖动, 保持原模式
    }
    let mut s = match app.state::<AppState>().settings.lock() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    s.float_mode = true;
    s.kb_pos = None;
    do_apply_settings(app, s);
}
/// 触摸板虚拟指针跟随线程 (30ms): 空闲时 V := 真实光标 —— 真实鼠标/画笔移动后,
/// 触摸板从新位置继续。手势期间不动 V (由 tp_move / 手势线程推进)。
/// 采样落在触摸板窗口内时跳过: 那是胶合"鬼影"位置, 不是用户的光标。
fn tp_cursor_poll(app: AppHandle) {
    let Ok(xconn) = xutil::XConn::new() else { return };
    loop {
        std::thread::sleep(Duration::from_millis(30));
        if app.state::<AppState>().tp_fingers.load(Ordering::Relaxed) > 0 {
            continue;
        }
        let Ok((x, y)) = xconn.cursor_pos() else { continue };
        if tp_in_pad_window(&app, x, y) {
            continue;
        }
        *app.state::<AppState>().tp_vpos.lock().unwrap() = Some((x, y));
    }
}

/// 坐标是否落在触摸板窗口内 (鬼影总在板上; 真实光标极少停在板上)
fn tp_in_pad_window(app: &AppHandle, x: f64, y: f64) -> bool {
    let Some(w) = app.get_webview_window("touchpad") else { return false };
    if !w.is_visible().unwrap_or(false) {
        return false;
    }
    if let (Ok(p), Ok(s)) = (w.outer_position(), w.outer_size()) {
        let (px, py) = (p.x as f64, p.y as f64);
        return x >= px && x < px + s.width as f64 && y >= py && y < py + s.height as f64;
    }
    false
}

/// 触摸板手势线程 (指针伺服 + touchmove 兜底):
/// - mutter 的触摸指针模拟把光标"胶"在手指上 (绝对吸附), 手势期间不能注入
///   相对移动 (会拉锯闪烁)。轮询路径: 被胶住的光标就是手指位置的镜像,
///   V += δ×增益, 手势期间零注入 (手指读数纯净)。
/// - touchmove 可用 (JS 发过 tp_move) 时: 手势期间伺服实时追 V (阻尼 0.4,
///   几何收敛 ~150ms, 有轻微跟随延迟但光标跟手)。
/// - 手势结束后的"回位窗口" (~500ms): 伺服把光标从板上鬼影位置带回 V;
///   平时不伺服 —— 空闲光标归真实鼠标管 (tp_cursor_poll 让 V 跟随真实光标)。
/// - 双触点 = 滚动 (锁定至全部抬起); 行程累计供 tp_click 轻触判定。
fn tp_motion_poll(app: AppHandle) {
    let Ok(xconn) = xutil::XConn::new() else { return };
    static DBG: OnceLock<bool> = OnceLock::new();
    let dbg = *DBG.get_or_init(|| std::env::var("TOUCH_KB_DEBUG").is_ok());
    let mut in_gesture = false;
    let mut wait_ticks = 0u32;
    let mut last_finger: Option<(f64, f64)> = None;
    let mut scroll_latch = false;
    let mut scroll_acc = 0.0f64;
    let mut natural = true;
    let mut return_until: Option<std::time::Instant> = None;
    loop {
        std::thread::sleep(Duration::from_millis(12));
        let state = app.state::<AppState>();
        let fingers = state.tp_fingers.load(Ordering::Relaxed);
        if fingers == 0 {
            if in_gesture {
                in_gesture = false;
                last_finger = None;
                scroll_latch = false;
                return_until = Some(std::time::Instant::now() + Duration::from_millis(500));
                if dbg {
                    eprintln!("[tp-motion] 手势结束, 回位窗口开启");
                }
            }
            // 回位窗口内把光标拉回 V (抬手后光标停在板上鬼影处)
            if return_until.is_some_and(|t| std::time::Instant::now() < t) {
                if tp_step_toward_v(&app, 0.5) {
                    return_until = None;
                }
            }
            continue;
        }
        if !in_gesture {
            in_gesture = true;
            wait_ticks = 0;
            last_finger = None;
            scroll_acc = 0.0;
            return_until = None;
            natural = state.settings.lock().unwrap().tp_natural_scroll;
            continue;
        }
        if state.tp_js_motion.load(Ordering::Relaxed) {
            // touchmove 可用: 前端更新 V, 这里伺服追赶 (跟手移动)
            let _ = tp_step_toward_v(&app, 0.4);
            continue;
        }
        // 轮询路径: 读被胶住的光标 = 手指位置 (期间零注入, 读数纯净)
        let Ok((cx, cy)) = xconn.cursor_pos() else { continue };
        wait_ticks += 1;
        if wait_ticks < 10 {
            continue; // 手势开始后等 ~120ms 给 touchmove 机会
        }
        if fingers >= 2 {
            scroll_latch = true;
        }
        if let Some((lx, ly)) = last_finger {
            let dx = cx - lx;
            let dy = cy - ly;
            if scroll_latch {
                // 双指滚动: 只累计纵向位移, 每 34px 一格 (自然滚动 = 内容跟随手指)
                scroll_acc += dy;
                let clicks = (scroll_acc / 34.0).trunc();
                if clicks != 0.0 {
                    scroll_acc -= clicks * 34.0;
                    let d = if natural { -clicks as i32 } else { clicks as i32 };
                    if let Ok(g) = state.mouse.lock() {
                        if let Some(b) = g.as_ref() {
                            let _ = b.scroll(0, d);
                        }
                    }
                    if dbg {
                        eprintln!("[tp-motion] scroll dy={d}");
                    }
                }
            } else {
                let gain = *state.tp_gain.lock().unwrap();
                {
                    let mut vg = state.tp_vpos.lock().unwrap();
                    if let Some((x, y)) = *vg {
                        *vg = Some((x + dx * gain, y + dy * gain));
                    }
                }
                state.tp_travel.fetch_add((dx.abs() + dy.abs()) as u32, Ordering::Relaxed);
                if dbg {
                    eprintln!("[tp-motion] f=({dx:.1},{dy:.1}) gain={gain:.2}");
                }
            }
        }
        last_finger = Some((cx, cy));
    }
}


/// 拖动结束: 吸附到最近的左右屏幕边缘 (工作区内), 更新悬浮球位置记录。
/// 贴右/下边缘时把球体锚定到窗口右/下角 (emit ball-corner 通知前端),
/// 窗口整体留在屏幕内 —— 否则 WM 会把要伸出屏幕的 200x200 窗口拉回, 球无法贴边。
fn snap_ball(app: &AppHandle) {
    let Some(ball) = app.get_webview_window("ball") else { return };
    let Ok(pos) = ball.outer_position() else { return };
    let m = ball
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = m else { return };
    let sf = m.scale_factor();
    let mut wx = m.position().x as f64;
    let mut wy = m.position().y as f64;
    let mut ww = m.size().width as f64;
    let mut wh = m.size().height as f64;
    if let Some((ax, ay, aw, ah)) = workarea_physical(app) {
        let l = wx.max(ax as f64);
        let t = wy.max(ay as f64);
        let r = (wx + ww).min(ax as f64 + aw as f64);
        let b = (wy + wh).min(ay as f64 + ah as f64);
        if r > l && b > t {
            wx = l;
            wy = t;
            ww = r - l;
            wh = b - t;
        }
    }
    let size = BALL_SIZE * sf;
    let margin = 6.0 * sf;
    let (wwin, hwin) = ball
        .outer_size()
        .map(|s| (s.width as f64, s.height as f64))
        .unwrap_or((200.0 * sf, 200.0 * sf));
    let to_left = pos.x as f64 + size / 2.0 < wx + ww / 2.0;
    let h_right = !to_left;
    let tx = if to_left { wx + margin } else { wx + ww - margin - wwin };
    // 垂直: 球体可视位置保持松手处 (夹取进工作区), 窗口按锚定角补偿避免伸出屏幕
    let tyv = (pos.y as f64).clamp(wy + margin, wy + wh - margin - size);
    let v_bottom = tyv + hwin > wy + wh - margin + 1.0;
    let ty = if v_bottom { tyv + size - hwin } else { tyv };
    *app.state::<AppState>().ball_pos.lock().unwrap() = Some((tx.round() as i32, ty.round() as i32));
    *app.state::<AppState>().ball_corner.lock().unwrap() = (h_right, v_bottom);
    let _ = ball.emit("ball-corner", (h_right, v_bottom));
    // 平滑吸附动画
    std::thread::spawn(move || {
        for i in 1..=8 {
            let x = pos.x + ((tx.round() as i32 - pos.x) * i) / 8;
            let y = pos.y + ((ty.round() as i32 - pos.y) * i) / 8;
            let _ = ball.set_position(PhysicalPosition::new(x, y));
            std::thread::sleep(Duration::from_millis(16));
        }
        let _ = ball.set_position(PhysicalPosition::new(tx.round() as i32, ty.round() as i32));
    });
}

/// 输入法引擎名 -> 托盘风格短标签 (≤2 字符)。
/// ibus 例: "xkb:us::eng"→En, "xkb:us::eng" 之外的 ::lang 取语言码; 中文引擎→中;
/// fcitx5 名称如 "键盘 - 英语 (美国)"→En、"拼音"→中。
fn ime_short_label(engine: &str) -> String {
    let e = engine.trim();
    let zh = ["拼音", "pinyin", "Pinyin", "bopomofo", "rime", "中州韵", "五笔", "仓颉", "cangjie", "wubi", "table:", "chewing", "sunpinyin", "libpinyin"];
    if zh.iter().any(|k| e.contains(k)) {
        return "中".into();
    }
    if ["mozc", "anthy", "skk", "kkc"].iter().any(|k| e.contains(k)) {
        return "あ".into();
    }
    if e.contains("hangul") {
        return "한".into();
    }
    if e.contains("英语") || e.contains("英文") || e.to_lowercase().contains("english") {
        return "En".into();
    }
    // ibus 布局源 "xkb:us::eng": 优先取 :: 后的语言码
    if let Some(lang) = e.split("::").nth(1) {
        let l = lang.trim();
        if !l.is_empty() {
            return capitalize2(l);
        }
    }
    if e.contains("日语") || e.contains("日文") || e.contains("japanese") {
        return "あ".into();
    }
    if e.contains("韩语") || e.contains("韩文") || e.contains("korean") {
        return "한".into();
    }
    // 兜底: CJK 名称取首字, 拉丁名取前两个字母
    let mut cs = e.chars();
    if let Some(c) = cs.next() {
        if (c as u32) >= 0x2E80 {
            return c.to_string();
        }
        return capitalize2(&e[..e.char_indices().nth(2).map(|(i, _)| i).unwrap_or(e.len())]);
    }
    "中".into()
}

fn capitalize2(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &cs.take(1).collect::<String>(),
        None => String::new(),
    }
}

/// 查询当前输入法: 优先 IBus (GNOME 默认), 回退 fcitx5。
/// 需要会话 D-Bus 环境; 都不可用时返回 None (前端保持现值)。
fn detect_ime_label() -> Option<String> {
    for (cmd, args) in [("ibus", vec!["engine"]), ("fcitx5-remote", vec!["-n"])] {
        if let Ok(out) = std::process::Command::new(cmd).args(args).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(ime_short_label(&s));
                }
            }
        }
    }
    None
}

/// 输入法标签轮询线程: 状态变化才 emit `ime-changed`;
/// 检测不可用时降频重试 (桌面会话缺失/未装输入法框架时避免空转)。
fn ime_poll(app: AppHandle) {
    let mut ok = false;
    let mut fail_logged = false;
    loop {
        if let Some(l) = detect_ime_label() {
            let changed;
            {
                let state = app.state::<AppState>();
                let mut st = state.ime_label.lock().unwrap();
                changed = *st != l;
                *st = l.clone();
            }
            if changed {
                eprintln!("[ime] 标签 -> {l}");
                let _ = app.emit("ime-changed", l);
            }
            if !ok {
                eprintln!("[ime] 检测可用");
            }
            ok = true;
            fail_logged = false;
        } else {
            if !fail_logged {
                eprintln!("[ime] 检测不可用 (ibus/fcitx5 均失败), 每 5s 重试");
                fail_logged = true;
            }
            ok = false;
        }
        std::thread::sleep(Duration::from_secs(if ok { 1 } else { 5 }));
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "显示 / 隐藏键盘", true, None::<&str>)?;
    let touchpad = CheckMenuItem::with_id(app, "touchpad", "触摸板", true, false, None::<&str>)?;
    let mode = MenuItem::with_id(app, "mode", "切换布局 (完整 / 分裂)", true, None::<&str>)?;
    let fit = MenuItem::with_id(app, "fit", "重新适配屏幕", true, None::<&str>)?;
    let ball = MenuItem::with_id(app, "ball", "最小化为悬浮球", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &toggle,
            &PredefinedMenuItem::separator(app)?,
            &touchpad,
            &mode,
            &fit,
            &ball,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    if let Ok(mut slot) = app.state::<AppState>().tray_tp.lock() {
        *slot = Some(touchpad);
    }
    TrayIconBuilder::with_id("tk-tray")
        .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?)
        .tooltip("触屏键盘 TouchKeyboard")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_keyboard(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => toggle_keyboard(app),
            "touchpad" => toggle_touchpad(app.clone()),
            "mode" => {
                let s = { app.state::<AppState>().settings.lock().unwrap().clone() };
                let mut s2 = s.clone();
                s2.mode = if s.mode == "split" { "full".to_string() } else { "split".to_string() };
                do_apply_settings(app, s2);
            }
            "fit" => refit(app),
            "ball" => hide_kb_show_ball(app),
            "settings" => toggle_settings(app.clone()),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn main() {
    // 有 XWayland 时强制走 X11: 获得窗口定位/置顶/点击穿透能力 (Wayland 原生窗口无这些 API)。
    // 按键注入在 Wayland 会话下走 uinput, 可到达包括原生 Wayland 在内的所有窗口。
    // 注意: 需覆盖会话可能预置的 GDK_BACKEND=wayland。
    if std::env::var("DISPLAY").is_ok() {
        std::env::set_var("GDK_BACKEND", "x11");
    }
    // 禁用 WebKitGTK 的 DMA-BUF 渲染器: 透明窗口上它会把只含损坏区域的新缓冲直接呈现,
    // 其余区域是未初始化的空内容 —— 快速点按时整窗闪黑 (帧差实测: 全窗像素清零、仅新气泡可见)。
    // 回退到旧渲染路径后无此问题; 键盘 UI 面积小, 性能损失可忽略。须在 webview 创建前设置。
    std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");

    tauri::Builder::default()
        .manage(AppState {
            input: Arc::new(Mutex::new(None)),
            mouse: Arc::new(Mutex::new(None)),
            tray_tp: Mutex::new(None),
            hit_regions: Mutex::new(HashMap::new()),
            click_through: AtomicBool::new(true),
            settings: Mutex::new(Settings::default()),
            ball_pos: Mutex::new(None),
            ball_corner: Mutex::new((true, false)),
            cursor_tracking: AtomicBool::new(false),
            ball_dragging: AtomicBool::new(false),
            ball_drag_active: AtomicBool::new(false),
            kb_dragging: AtomicBool::new(false),
            kb_drag_active: AtomicBool::new(false),
            xconn: Mutex::new(None),
            xids: Mutex::new(HashMap::new()),
            kb_float_w: Mutex::new(0.0),
            ime_label: Mutex::new("中".into()),
            tp_vpos: Mutex::new(None),
            tp_fingers: AtomicU8::new(0),
            tp_js_motion: AtomicBool::new(false),
            tp_travel: AtomicU32::new(0),
            tp_gain: Mutex::new(2.0),
            update: Mutex::new(UpdateMem::default()),
        })
        .invoke_handler(tauri::generate_handler![
            get_screen_info,
            update_hit_regions,
            set_click_through,
            tap_key,
            input_backend_name,
            get_ime_label,
            apply_settings,
            minimize_to_ball,
            restore_keyboard,
            ball_moved,
            start_ball_drag,
            end_ball_drag,
            ball_input_mode,
            start_kb_drag,
            end_kb_drag,
            toggle_settings,
            hide_settings,
            toggle_touchpad,
            tp_surface,
            tp_move,
            tp_drag,
            tp_click,
            tp_scroll,
            mouse_backend_name,
            get_settings,
            get_prefers_dark,
            start_icon_url,
            check_update,
            app_version,
            open_url,
            quit_app
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            match input::create_backend() {
                Ok(b) => {
                    println!("[touch-keyboard] 输入后端: {}", b.name());
                    *handle.state::<AppState>().input.lock().unwrap() = Some(b);
                }
                Err(e) => eprintln!("[touch-keyboard] 输入后端不可用: {e}"),
            }

            match input::mouse::create_mouse_backend() {
                Ok(b) => {
                    println!("[touch-keyboard] 鼠标后端: {}", b.name());
                    *handle.state::<AppState>().mouse.lock().unwrap() = Some(b);
                }
                Err(e) => eprintln!("[touch-keyboard] 鼠标后端不可用: {e}"),
            }

            // 焦点: 键盘/悬浮球/设置窗口均不抢焦点 (GTK 权威设置 WM_HINTS)。
            // 设置窗口不抢焦点 = 调整设置时目标应用焦点不变, 键盘仍可打字
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            {
                if let Some(kb) = handle.get_webview_window("keyboard") {
                    disable_focus(&kb);
                }
                if let Some(ball) = handle.get_webview_window("ball") {
                    disable_focus(&ball);
                }
                if let Some(sw) = handle.get_webview_window("settings") {
                    disable_focus(&sw);
                }
                if let Some(tp) = handle.get_webview_window("touchpad") {
                    disable_focus(&tp);
                }
            }

            refit(&handle);
            if let Some(kb) = handle.get_webview_window("keyboard") {
                let _ = kb.show();
            }
            if let Some(ball) = handle.get_webview_window("ball") {
                // GTK 可能放大窗口到 200x200, 显式定尺寸
                let _ = ball.set_size(LogicalSize::new(BALL_SIZE, BALL_SIZE));
                let pos = default_ball_pos(&handle);
                *handle.state::<AppState>().ball_pos.lock().unwrap() = Some(pos);
                let _ = ball.set_position(PhysicalPosition::new(pos.0, pos.1));
            }

            // X11: 查找键盘窗口 xid → 不抢焦点 (WM_HINTS) + 输入区域。
            // (悬浮球的 X 窗口首次显示时才创建, 其输入区域在 show 时应用)
            {
                let h = handle.clone();
                std::thread::spawn(move || {
                    let Ok(x) = xutil::XConn::new() else {
                        eprintln!("[touch-keyboard] XConn 创建失败, 无法应用输入区域");
                        return;
                    };
                    println!("[touch-keyboard] 开始查找 X 窗口...");
                    for _i in 0..40 {
                        if let Some(kb_id) = x.find_window_id("触屏键盘") {
                            h.state::<AppState>().xids.lock().unwrap()
                                .insert("keyboard".into(), kb_id);
                            // 键盘/悬浮球/设置/触摸板窗口一并设为不抢焦点
                            let _ = x.relax_focus(&["触屏键盘", "悬浮球", "键盘设置", "触摸板"]);
                            let r = apply_input_shape(&h, "keyboard");
                            println!("[touch-keyboard] 键盘输入区域已应用 kb={kb_id} result={r:?}");
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                });
            }

            // 悬浮球/键盘拖动跟随线程 (需要 X 全局光标)
            let cursor_ok = xutil::XConn::new().is_ok();
            handle.state::<AppState>().cursor_tracking.store(cursor_ok, Ordering::Relaxed);
            if cursor_ok {
                let h = handle.clone();
                std::thread::spawn(move || ball_drag_poll(h));
                let h2 = handle.clone();
                std::thread::spawn(move || kb_drag_poll(h2));
                let h3 = handle.clone();
                std::thread::spawn(move || tp_cursor_poll(h3));
                let h4 = handle.clone();
                std::thread::spawn(move || tp_motion_poll(h4));
            }

            // 输入法标签轮询线程 (随系统托盘/输入源指示同步)
            {
                let h = handle.clone();
                std::thread::spawn(move || ime_poll(h));
            }

            // 系统深浅色轮询线程 (主题 = 跟随系统时前端即时切换)
            {
                let h = handle.clone();
                std::thread::spawn(move || theme_poll(h));
            }

            // 新版本检测线程 (启动 8s 后首查, 之后每天一次; 失败静默)
            {
                let h = handle.clone();
                std::thread::spawn(move || update_loop(h));
            }

            build_tray(&handle)?;

            // 调试/自启动用: TOUCH_KB_TP_SHOW=1 启动即显示触摸板
            if std::env::var("TOUCH_KB_TP_SHOW").is_ok() {
                show_touchpad(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                // 点关闭 = 隐藏窗口, 程序常驻后台
                api.prevent_close();
                match window.label() {
                    "settings" => {
                        let _ = window.hide();
                    }
                    "touchpad" => hide_touchpad(window.app_handle()),
                    _ => hide_kb_show_ball(window.app_handle()),
                }
            }
            // 触摸板窗口位置记录: 只记可见时的移动 (隐藏/启动期 GTK 摆位不算用户意图),
            // 隐藏时随 settings-changed 回流前端持久化
            WindowEvent::Moved(p) => {
                if window.label() == "touchpad" && window.is_visible().unwrap_or(false) {
                    if let Ok(mut s) = window.app_handle().state::<AppState>().settings.lock() {
                        s.tp_pos = Some([p.x, p.y]);
                    }
                }
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("touch-keyboard 运行失败");
}

// ---- 新版本检测 (updater.rs): 后台检查线程 + 手动检查命令 ----
// Linux 没有安全的自动安装路径 (deb 需 root), 只做检测: 发现新版本后
// emit `update-available` 给全部窗口 (键盘页 ⚙ 角标 + 设置窗口卡片),
// 下载由用户在 Release 页面手动完成 (open_url 用系统浏览器打开)。

/// 前端 "update-available" 事件载荷
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAvailable {
    current_version: String,
    new_version: String,
    release_url: String,
    notes: String,
}

/// 手动检查 (check_update 命令) 的同步返回: 设置窗口据此做轻提示;
/// 有更新时卡片由 "update-available" 事件渲染
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "kind")]
enum CheckOutcome {
    UpToDate { current: String },
    Available { current: String, new_version: String, release_url: String },
    Failed { message: String },
}

fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Release 说明截断 (按字符计, 防超长 body 撑爆设置窗口; 前端另有 max-height)
fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// 执行一次检查 (手动/自动共用): 取到 Release 才记 last_check (网络失败
/// 不记, 下个小时重试); 有新版本时 emit 全部窗口。自动路径失败静默,
/// 失败结果只返回给手动调用方提示
fn do_check(app: &AppHandle) -> CheckOutcome {
    let state = app.state::<AppState>();
    {
        let mut u = state.update.lock().unwrap();
        if u.checking {
            return CheckOutcome::Failed { message: "已有检查正在进行".into() };
        }
        u.checking = true;
    }
    let current = current_version(app);
    let outcome = match updater::fetch_latest(&format!("touch-keyboard/{current}")) {
        None => CheckOutcome::Failed { message: "网络异常或 Release 信息不可用".into() },
        Some(rel) => {
            {
                let mut u = state.update.lock().unwrap();
                u.last_check_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
            }
            if updater::is_newer(&rel.tag, &current) {
                let _ = app.emit(
                    "update-available",
                    UpdateAvailable {
                        current_version: current.clone(),
                        new_version: rel.version.clone(),
                        release_url: rel.url.clone(),
                        notes: truncate_chars(&rel.notes, 400),
                    },
                );
                let new_version = rel.version.clone();
                let release_url = rel.url.clone();
                state.update.lock().unwrap().latest = Some(rel);
                CheckOutcome::Available { current, new_version, release_url }
            } else {
                CheckOutcome::UpToDate { current }
            }
        }
    };
    state.update.lock().unwrap().checking = false;
    outcome
}

/// 后台更新检查线程: 启动延迟 8s (避开启动期 X 连接/轮询线程高峰)先查一次;
/// 之后每小时醒一次, 距上次成功检查 ≥24h 才真正发请求 (每天一次)。
/// 失败在 fetch_latest 内部吞掉, 线程永不打扰用户
fn update_loop(app: AppHandle) {
    std::thread::sleep(Duration::from_secs(8));
    let _ = do_check(&app);
    loop {
        std::thread::sleep(Duration::from_secs(3600));
        let due = {
            let state = app.state::<AppState>();
            let u = state.update.lock().unwrap();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            now - u.last_check_ms >= 24 * 3600 * 1000
        };
        if due {
            let _ = do_check(&app);
        }
    }
}

/// 手动检查 (设置窗口「检查更新」按钮): 同步返回结果给前端做轻提示;
/// 有更新时卡片由 "update-available" 事件渲染 (本命令只负责结果提示)
#[tauri::command]
fn check_update(app: AppHandle) -> CheckOutcome {
    do_check(&app)
}

/// 当前版本号 (设置窗口「关于」区显示, 来源 tauri.conf.json)
#[tauri::command]
fn app_version(app: AppHandle) -> String {
    current_version(&app)
}

/// 用系统默认浏览器打开链接 (新版本 Release 页面)。WebView 内 <a> 导航
/// 行为不可控, 统一由后端代开; 仅接受 https, 防注入 file:// 一类协议
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("仅支持 https 链接".into());
    }
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(not(target_os = "linux"))]
    let cmd = {
        let _ = &url;
        return Err("当前平台不支持".into());
    };
    std::process::Command::new(cmd)
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开浏览器失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{b64_encode, ime_short_label};

    #[test]
    fn ime_labels() {
        assert_eq!(ime_short_label("xkb:us::eng"), "En");
        assert_eq!(ime_short_label("xkb:jp::jpn"), "Jp");
        assert_eq!(ime_short_label("libpinyin"), "中");
        assert_eq!(ime_short_label("pinyin"), "中");
        assert_eq!(ime_short_label("ibus-table-cangjie"), "中");
        assert_eq!(ime_short_label("mozc"), "あ");
        assert_eq!(ime_short_label("hangul"), "한");
        assert_eq!(ime_short_label("键盘 - 英语 (美国)"), "En");
        assert_eq!(ime_short_label("拼音"), "中");
        assert_eq!(ime_short_label("rime"), "中");
    }

    #[test]
    fn base64() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
    }
}
