/* 屏幕触摸板前端: 手势识别 (触点数上报 + touchmove 位移上报, 可用时)。
   - 手势全部走 touch 事件 (WebKitGTK 真实触摸的可靠路径, 见 AGENTS 机制 3);
     表面/左右按键有意忽略鼠标 pointer 事件 —— 注入的指针会落到自家窗口形成反馈回路。
   - 指针移动/点击的主体在后端 (虚拟指针 V + 画笔绝对跳变, 见 AGENTS 机制 13):
     mutter 的触摸指针模拟把光标"胶"在手指上, 不能对抗只能利用 —— 前端只上报
     触点数 (tp_surface) 与手指位移 (tp_move, 增益在后端)。
   - 单指移动 / 轻触单击 / 长按 ~350ms 再滑动 = 拖动 / 双指滚动 (自然滚动) /
     双指轻触右键 / 三指轻触中键; 左右按键 = 轻触完整点击。
   - 设置与键盘/设置窗口共享 localStorage (tk-settings-v1), 主题随全局切换。 */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window.getCurrentWindow();

const LS_KEY = 'tk-settings-v1';
const DEFAULTS = {
  mode: 'full', kb_scale: 1.0, opacity: 0.95, num_row: true, click_through: true,
  float_mode: false, kb_pos: null, float_width: 0.6, split_half_units: 6.8, theme: 'auto',
  bubble: true, tp_speed: 1.0, tp_tap_click: true, tp_natural_scroll: true, tp_pos: null,
};
const SETTING_KEYS = Object.keys(DEFAULTS);
let S = { ...DEFAULTS, ...JSON.parse(localStorage.getItem(LS_KEY) || '{}') };
S.theme = ['light', 'dark', 'auto'].includes(S.theme) ? S.theme : 'auto';
S.tp_speed = clampSpeed(S.tp_speed);
let prefersDark = false;

function clampSpeed(v) {
  const n = Number(v);
  return Number.isFinite(n) ? Math.min(3, Math.max(0.3, n)) : 1.0;
}

function save() {
  const out = {};
  SETTING_KEYS.forEach(k => { out[k] = S[k]; });
  localStorage.setItem(LS_KEY, JSON.stringify(out));
}

function applyTheme() {
  const dark = S.theme === 'dark' || (S.theme === 'auto' && prefersDark);
  document.documentElement.classList.toggle('dark', dark);
}

function showBanner(msg) {
  const b = document.getElementById('banner');
  b.textContent = msg;
  b.classList.remove('hidden');
  setTimeout(() => b.classList.add('hidden'), 6000);
}

// ---- 屏幕信息 (增益计算在后端; 这里只取 scale_factor 换算物理像素) ----
let screen = null;
const surface = document.getElementById('tp-surface');

function sf() {
  return (screen && screen.scale_factor) || 1;
}

// ---- 注入 ----
// lastInject: 最近一次注入时间戳。注入的指针事件可能落回自家窗口 (顶栏/✕),
// 鼠标 pointer 事件在注入后 400ms 内一律忽略, 防止"点按自家窗口被误触"。
let lastInject = 0;

// 移动按帧合帧: 累计小数残差, 每帧发整数物理像素
let accX = 0, accY = 0, moveQueued = false;

function queueMove(dx, dy) {
  accX += dx;
  accY += dy;
  if (!moveQueued) {
    moveQueued = true;
    requestAnimationFrame(flushMove);
  }
}

function flushMove() {
  moveQueued = false;
  const ix = Math.trunc(accX), iy = Math.trunc(accY);
  accX -= ix;
  accY -= iy;
  if (ix || iy) {
    lastInject = Date.now();
    invoke('tp_move', { dx: ix, dy: iy }).catch(() => {});
  }
}

// ---- 表面手势 ----
// 指针移动/点击的主体在后端 (虚拟指针 V + 画笔绝对跳变, 见 AGENTS 机制 13):
// mutter 的触摸指针模拟把光标"胶"在手指上, 前端只上报触点数与手指位移。
const TAP_MS = 280;      // 轻触最长按压时长
const TAP_DIST = 14;     // 轻触最大位移 (px)
const DRAG_MS = 350;     // 长按进入拖动的时长
const SCROLL_START = 14; // 双指滚动进入阈值 (防止双指轻触被算成滚动)
const WHEEL_PX = 34;     // 每滚轮格的手指位移

const contacts = new Map(); // touch.identifier -> { x, y, travel }
let gestureStart = 0, maxFingers = 0, movedFar = false, scrollMode = false, scrolledFar = false;
let scrollAccX = 0, scrollAccY = 0;
let dragTimer = null, dragging = false;

// 手势期间隐藏光标: mutter 把光标"胶"在手指上 (鬼影落在板内), 隐藏之;
// 抬手后后端伺服把光标拉回 V 时恢复可见 (平时光标正常显示)
function setGhostHidden(on) {
  document.documentElement.classList.toggle('gesturing', on);
}

function cancelDragTimer() {
  if (dragTimer) { clearTimeout(dragTimer); dragTimer = null; }
}

function flushScroll() {
  const cx = Math.trunc(scrollAccX / WHEEL_PX);
  const cy = Math.trunc(scrollAccY / WHEEL_PX);
  if (cx) scrollAccX -= cx * WHEEL_PX;
  if (cy) scrollAccY -= cy * WHEEL_PX;
  if (cx || cy) {
    // 自然滚动: 内容跟随手指, 滚轮方向与传统相反
    const sign = S.tp_natural_scroll ? -1 : 1;
    lastInject = Date.now();
    invoke('tp_scroll', { dx: sign * cx, dy: sign * cy }).catch(() => {});
  }
}

surface.addEventListener('touchstart', e => {
  e.preventDefault();
  if (contacts.size === 0) { // 新一轮手势
    gestureStart = Date.now();
    maxFingers = 0;
    movedFar = false;
    scrollMode = false;
    scrolledFar = false;
    scrollAccX = 0;
    scrollAccY = 0;
    dragging = false;
    // 长按不动 → 进入拖动 (按住左键滑动, 松手结束)
    dragTimer = setTimeout(() => {
      dragTimer = null;
      if (contacts.size === 1 && !scrollMode) {
        dragging = true;
        surface.classList.add('dragging');
        lastInject = Date.now();
        invoke('tp_drag', { press: true }).catch(() => {});
      }
    }, DRAG_MS);
  }
  for (const t of e.changedTouches) {
    contacts.set(t.identifier, { x: t.clientX, y: t.clientY, travel: 0 });
  }
  maxFingers = Math.max(maxFingers, contacts.size);
  if (contacts.size >= 2) cancelDragTimer(); // 双指 = 滚动, 取消长按拖动
  surface.classList.add('touching');
  setGhostHidden(true);
  // 触点数上报: 0→≥1 手势开始 (后端初始化虚拟指针/增益), →0 手势结束 (回位窗口拉回 V)
  invoke('tp_surface', { fingers: Math.min(contacts.size, 5) }).catch(() => {});
}, { passive: false });

surface.addEventListener('touchmove', e => {
  e.preventDefault();
  let sumDx = 0, sumDy = 0, moved = 0;
  for (const t of e.changedTouches) {
    const c = contacts.get(t.identifier);
    if (!c) continue;
    const dx = t.clientX - c.x;
    const dy = t.clientY - c.y;
    c.x = t.clientX;
    c.y = t.clientY;
    c.travel += Math.abs(dx) + Math.abs(dy);
    sumDx += dx;
    sumDy += dy;
    moved++;
  }
  if (!moved) return;
  const total = contacts.size;
  if (total >= 2 || scrollMode) {
    // 双指滚动 (滚动中抬起一指, 剩余手指继续滚动)
    scrollMode = true;
    const n = total >= 2 ? moved : 1;
    scrollAccX += sumDx / n;
    scrollAccY += sumDy / n;
    if (Math.abs(scrollAccX) + Math.abs(scrollAccY) > SCROLL_START) scrolledFar = true;
    flushScroll();
  } else {
    // 单指移动指针 (增益在后端: 工作区宽/板宽 × tp_speed)
    const c = contacts.values().next().value;
    if (c && c.travel > TAP_DIST) {
      movedFar = true;
      cancelDragTimer(); // 已开始移动则不进入长按拖动 (拖动 = 按住不动 350ms 后再滑)
    }
    queueMove(sumDx * sf(), sumDy * sf());
  }
}, { passive: false });

async function onTouchEnd(e) {
  for (const t of e.changedTouches) contacts.delete(t.identifier);
  if (contacts.size > 0) {
    // 还有手指在板上: 更新触点数 (滚动中抬起一指等)
    invoke('tp_surface', { fingers: Math.min(contacts.size, 5) }).catch(() => {});
    return;
  }
  cancelDragTimer();
  surface.classList.remove('touching');
  surface.classList.remove('dragging');
  setGhostHidden(false);
  const dur = Date.now() - gestureStart;
  if (dragging) {
    // 拖动结束: 松开左键
    dragging = false;
    lastInject = Date.now();
    try { await invoke('tp_drag', { press: false }); } catch (err) { /* 忽略 */ }
  } else if (S.tp_tap_click && !movedFar && !scrolledFar && dur < TAP_MS + 80) {
    const btn = maxFingers === 1 ? 1 : maxFingers === 2 ? 3 : 2; // 1左 3右 2中
    lastInject = Date.now();
    try { await invoke('tp_click', { btn }); } catch (err) { /* 忽略 */ }
  }
  // 手势结束 (后端跳回 V 恢复可见光标)
  invoke('tp_surface', { fingers: 0 }).catch(() => {});
}
surface.addEventListener('touchend', onTouchEnd);
surface.addEventListener('touchcancel', onTouchEnd);

// ---- 左右实体按键: 轻触 = 对应键完整点击 (拖动用表面长按手势) ----
function bindBtn(el, btn) {
  let down = 0;
  el.addEventListener('touchstart', e => {
    e.preventDefault();
    down = Date.now();
    el.classList.add('pressing');
    setGhostHidden(true); // 按键触摸同样会把光标胶在板上, 隐藏鬼影
  }, { passive: false });
  const up = () => {
    if (!down) return;
    const dur = Date.now() - down;
    down = 0;
    el.classList.remove('pressing');
    setGhostHidden(false);
    if (dur < 500) {
      lastInject = Date.now();
      invoke('tp_click', { btn }).catch(() => {});
    }
  };
  el.addEventListener('touchend', up);
  el.addEventListener('touchcancel', up);
}
bindBtn(document.getElementById('tp-btn-l'), 1);
bindBtn(document.getElementById('tp-btn-r'), 3);

// ---- 顶栏: 拖动窗口 / 关闭 (触摸 + 鼠标, 鼠标需过注入防误触窗口) ----
const guardMouse = () => Date.now() - lastInject < 400;
let lastHeadDown = 0, lastCloseDown = 0;

function headDown(e) {
  if (e.target.closest('#tp-close')) return;
  if (e.pointerType === 'mouse' && guardMouse()) return;
  e.preventDefault();
  if (Date.now() - lastHeadDown < 500) return; // pointer/touch 去重
  lastHeadDown = Date.now();
  win.startDragging().catch(() => {});
}
const head = document.getElementById('tp-head');
head.addEventListener('pointerdown', headDown);
head.addEventListener('touchstart', headDown, { passive: false });

function closeDown(e) {
  e.preventDefault();
  if (e.pointerType === 'mouse' && guardMouse()) return;
  if (Date.now() - lastCloseDown < 300) return;
  lastCloseDown = Date.now();
  invoke('toggle_touchpad').catch(err => showBanner(String(err)));
}
const closeBtn = document.getElementById('tp-close');
closeBtn.addEventListener('pointerdown', closeDown);
closeBtn.addEventListener('touchstart', closeDown, { passive: false });

document.addEventListener('contextmenu', e => e.preventDefault());
// 兜底: 元素级 touchend 丢失时也恢复光标可见
window.addEventListener('touchend', e => { if (!e.touches.length) setGhostHidden(false); });
window.addEventListener('touchcancel', () => setGhostHidden(false));

// ---- 初始化 ----
(async () => {
  applyTheme();
  try { screen = await invoke('get_screen_info'); } catch (e) { /* 增益走默认值 */ }
  invoke('mouse_backend_name')
    .then(n => {
      if (n === 'none') showBanner('⚠ 鼠标后端不可用: 触摸板需要 input 组权限 (uinput) 或 X11 会话');
    })
    .catch(() => {});
  invoke('get_prefers_dark')
    .then(d => { prefersDark = !!d; applyTheme(); })
    .catch(() => {});
  await listen('prefers-dark-changed', e => {
    prefersDark = !!e.payload;
    applyTheme();
  });
  // 键盘页/设置窗口/托盘改设置后的回显: 合并持久化 (主题/速度/滚动方向即时生效)
  await listen('settings-changed', e => {
    const p = e.payload || {};
    let dirty = false;
    SETTING_KEYS.forEach(k => {
      if (p[k] !== undefined && JSON.stringify(S[k]) !== JSON.stringify(p[k])) {
        S[k] = p[k];
        dirty = true;
      }
    });
    if (!dirty) return;
    S.tp_speed = clampSpeed(S.tp_speed);
    save();
    applyTheme();
  });
})();
