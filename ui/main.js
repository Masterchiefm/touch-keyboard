/* 触屏键盘前端: 布局渲染、按键处理、一次性修饰键、按键气泡、点击穿透区域上报。
   设置面板是独立窗口 (settings.html), 本页只消费 `settings-changed` 事件并回推修改。 */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const LS_KEY = 'tk-settings-v1';
const DEFAULTS = {
  mode: 'full', kb_scale: 1.0, opacity: 0.95, num_row: true, click_through: true,
  float_mode: false, kb_pos: null, float_width: 0.6, split_half_units: 6.8, theme: 'auto',
  bubble: true, tp_speed: 1.0, tp_tap_click: true, tp_natural_scroll: true, tp_pos: null,
};
const SETTING_KEYS = Object.keys(DEFAULTS);
let S = { ...DEFAULTS, ...JSON.parse(localStorage.getItem(LS_KEY) || '{}') };
S.theme = ['light', 'dark', 'auto'].includes(S.theme) ? S.theme : 'auto';
S.split_half_units = clampSplitUnits(S.split_half_units);
S.caps = false;
S.ime_label = '中'; // 输入法短标签, 由后端轮询 ibus/fcitx5 推送, 不持久化
S.mods = { shift: false, ctrl: false, alt: false };
let screen = { logical_width: innerWidth, logical_height: 400, scale_factor: 1 };
let lastShiftTap = 0;
let updAvailable = false; // 检测到新版本 (后端 update-available 事件): ⚙ 角标提示

const kbEl = document.getElementById('kb');
const appEl = document.getElementById('app');

function clampSplitUnits(v) {
  const n = Number(v);
  return Number.isFinite(n) ? Math.min(9.5, Math.max(5.0, n)) : 6.8;
}

function save() {
  const out = {};
  SETTING_KEYS.forEach(k => { out[k] = S[k]; });
  localStorage.setItem(LS_KEY, JSON.stringify(out));
}

// 主题功能上线前的旧默认是「浅色」, 一次性迁移为「跟随系统」;
// 之后用户手动选的浅色不再被迁移 (标记位只生效一次)
try {
  if (!localStorage.getItem('tk-theme-migrated')) {
    localStorage.setItem('tk-theme-migrated', '1');
    if (S.theme === 'light') { S.theme = 'auto'; save(); }
  }
} catch (e) { /* 忽略 */ }

// 推送给后端的设置 (须与 Rust Settings 字段一致)
function cleanSettings() {
  const out = {};
  SETTING_KEYS.forEach(k => { out[k] = S[k]; });
  return out;
}

// ---- 主题 (light / dark / 跟随系统) ----
let prefersDark = false;

function applyTheme() {
  const dark = S.theme === 'dark' || (S.theme === 'auto' && prefersDark);
  document.documentElement.classList.toggle('dark', dark);
}

const k = (label, code, opts = {}) => ({ label, code, ...opts });
// 上档符号键: 直接点发主字符, Shift/Caps 激活时发上档字符 (如 1→!, ;→:)
const sup = (l, c, s, opts = {}) => k(l, c, { sup: s, ...opts });

function fullRows() {
  // Windows 屏幕键盘布局: 各行权重合计恒为 15.55u, 跨行键宽严格对齐;
  // 数字行为矮行 (CSS .row-nums, 0.68 倍行高), Esc/Tab/Caps/Shift/⌫/Del/Enter 灰色,
  // 字符键与空格白色, 空格键帽留白 (同 Windows)
  return [
    [
      k('Esc', 'escape', { w: 1, cls: 'mod' }),
      sup('`', '`', '~'),
      ...'1234567890'.split('').map((c, i) => sup(c, c, '!@#$%^&*()'[i])),
      sup('-', '-', '_'), sup('=', '=', '+'),
      k('⌫', 'backspace', { w: 1.55, cls: 'mod', repeat: 1 }),
    ],
    [
      k('Tab', 'tab', { w: 1.55, cls: 'mod' }),
      ...'qwertyuiop'.split('').map(c => k(c, c)),
      sup('[', '[', '{'), sup(']', ']', '}'), sup('\\', '\\', '|'),
      k('Del', 'delete', { cls: 'mod', repeat: 1 }),
    ],
    [
      k('Caps', 'capslock', { w: 1.9, type: 'caps' }),
      ...'asdfghjkl'.split('').map(c => k(c, c)),
      sup(';', ';', ':'), sup("'", "'", '"'),
      k('↵', 'enter', { w: 2.65, cls: 'mod' }),
    ],
    [
      k('Shift', 'shift', { type: 'shift', w: 2.4 }),
      ...'zxcvbnm'.split('').map(c => k(c, c)),
      sup(',', ',', '<'), sup('.', '.', '>'), sup('/', '/', '?'),
      k('↑', 'up', { repeat: 1, cls: 'mod' }),
      k('Shift', 'shift', { type: 'shift', w: 2.15 }),
    ],
    [
      k('Ctrl', 'ctrl', { type: 'mod' }),
      k('Alt', 'alt', { type: 'mod' }),
      k('开始', 'start', { type: 'start', w: 1.55 }),
      k('', 'space', { w: 5.85 }),
      k('Alt', 'alt', { type: 'mod' }),
      k('Ctrl', 'ctrl', { type: 'mod' }),
      k('←', 'left', { repeat: 1, cls: 'mod' }),
      k('↓', 'down', { repeat: 1, cls: 'mod' }),
      k('→', 'right', { repeat: 1, cls: 'mod' }),
      k(S.ime_label || '中', 'ime', { type: 'ime', w: 1.15 }),
    ],
  ];
}

function splitRows() {
  const L = [], R = [];
  const nums = '1234567890'.split('');
  if (S.num_row) {
    L.push(nums.slice(0, 5).map(c => k(c, c)));
    R.push(nums.slice(5).map(c => k(c, c)));
  }
  L.push('qwert'.split('').map(c => k(c, c)));
  L.push('asdfg'.split('').map(c => k(c, c)));
  L.push([k('Shift', 'shift', { type: 'shift', w: 1.5 }), ...'zxcvb'.split('').map(c => k(c, c))]);
  L.push([
    k('Ctrl', 'ctrl', { type: 'mod' }),
    k('开始', 'start', { type: 'start' }),
    k('', 'space', { w: 3 }),
  ]);
  R.push('yuiop'.split('').map(c => k(c, c)));
  R.push('hjkl;'.split('').map(c => k(c, c)));
  R.push([...['n', 'm', ',', '.', '/'].map(c => k(c, c)), k('Shift', 'shift', { type: 'shift', w: 1.5 })]);
  R.push([
    k('Alt', 'alt', { type: 'mod' }),
    k(S.ime_label || '中', 'ime', { type: 'ime' }),
    k('↵', 'enter', { w: 3, cls: 'mod' }),
  ]);
  // 退格固定在右半面板首行右上角 (与完整布局一致)
  R[0].push(k('⌫', 'backspace', { w: 1, cls: 'mod', repeat: 1 }));
  return { L, R };
}

const esc = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

// ---- 「开始」键图标: 系统图标 (start-here), 无则内置 Linux 企鹅, 再退回文字 ----
// undefined = 还在加载 (先显示文字, 加载完重绘); null = 都没有, 用文字;
// 字符串 = 键帽内联 HTML (系统图标 data URL 或 tux.svg)
let startIcon;

async function loadStartIcon() {
  try {
    const url = await invoke('start_icon_url');
    if (url) startIcon = `<img class="start-img" src="${esc(url)}" alt="" draggable="false">`;
  } catch (e) { /* 忽略, 走企鹅兜底 */ }
  if (!startIcon) {
    try {
      const svg = await fetch('tux.svg').then(r => r.ok ? r.text() : Promise.reject(r.status));
      if (svg.includes('<svg')) startIcon = `<span class="start-svg">${svg}</span>`;
    } catch (e) { /* 忽略, 走文字兜底 */ }
  }
  render();
}

function keyHtml(kk) {
  const w = kk.w || 1;
  const isMod = kk.type === 'mod' || kk.type === 'shift' || kk.type === 'caps' || kk.type === 'ime' || kk.type === 'start';
  const cls = ['key', kk.cls || '', isMod ? 'mod' : '', kk.sup ? 'dual' : ''].filter(Boolean).join(' ');
  let inner;
  if (kk.type === 'start') {
    // 开始键: 系统图标 → Linux 企鹅 → 文字
    inner = `<span class="start-ic">${startIcon || esc(kk.label)}</span>`;
  } else if (kk.sup) {
    inner = `<span class="sup">${esc(kk.sup)}</span><span class="main">${esc(kk.label)}</span>`;
  } else {
    inner = esc(kk.label);
  }
  return `<div class="${cls}" style="--w:${w}" data-code="${esc(kk.code)}"` +
    ` data-shift="${kk.shift ? 1 : 0}" data-type="${kk.type || 'char'}"` +
    `${kk.repeat ? ' data-repeat="1"' : ''}>${inner}</div>`;
}

// 行内采用「步距模型」: 每键占位 = 权重 × (行宽/总权重), 键间隔含在步距内,
// 累计权重相同的键边界跨行像素级对齐 (如 完整模式 ↑ 与 ↓ 同列)
const rowHtml = (keys, cls) => {
  const total = keys.reduce((s, kk) => s + (kk.w || 1), 0);
  return `<div class="row${cls ? ' ' + cls : ''}" style="--total:${total.toFixed(4)}">${keys.map(keyHtml).join('')}</div>`;
};

function render() {
  kbEl.style.opacity = S.opacity; // 仅键盘半透明, 设置窗口独立不受影响
  kbEl.dataset.mode = S.mode;
  // 分裂宽度: 两半面板各占 su 个键距 (CSS 以 --su 计算面板宽与键距)
  appEl.style.setProperty('--su', String(clampSplitUnits(S.split_half_units)));
  const grip = `<div class="grip" data-type="drag"></div>`;
  // upd 类 = 有新版本 (update-available 事件置位): ⚙ 右上角红点提示,
  // 整树 render 会重建 DOM, 因此角标状态放进 HTML 而非事后补挂
  const settingsBtn = `<div class="key tbtn${updAvailable ? ' upd' : ''}" data-act="settings" data-type="act" title="设置">⚙</div>`;
  const closeBtn = `<div class="key tbtn" data-act="close" data-type="act" title="最小化到悬浮球">✕</div>`;
  const modeBtn = `<div class="key tbtn wide" data-act="mode" data-type="act" title="切换布局">${S.mode === 'full' ? '分裂' : '完整'}</div>`;
  const dockBtn = `<div class="key tbtn wide" data-act="${S.float_mode ? 'dock' : 'float'}" data-type="act" title="窗口模式">${S.float_mode ? '停靠' : '悬浮'}</div>`;
  const rowsHtml = rows => rows.map(r => rowHtml(r)).join('');
  let html;
  if (S.mode === 'full') {
    // 完整模式: 顶部共享标题栏 (⚙|把手|✕) + 工具条 (布局/窗口模式+弹簧把手), 键区单面板
    const titlebar = `<div class="titlebar">${settingsBtn}${grip}${closeBtn}</div>`;
    const toolbar = `<div class="toolbar">${modeBtn}${dockBtn}${grip}</div>`;
    const rows = fullRows().map((r, i) => rowHtml(r, i === 0 ? 'row-nums' : '')).join('');
    html = titlebar + toolbar + `<div class="panel">${rows}</div>`;
  } else {
    // 分裂模式: 两半各自独立面板, 各带小标题栏 (左: ⚙|布局|把手, 右: 把手|窗口|✕), 中缝穿透
    const { L, R } = splitRows();
    const half = (rows, left) => `<div class="panel half"><div class="titlebar">` +
      (left ? settingsBtn + modeBtn + grip : grip + dockBtn + closeBtn) +
      `</div>${rowsHtml(rows)}</div>`;
    html = `<div class="split-wrap">${half(L, true)}${half(R, false)}</div>`;
  }
  kbEl.innerHTML = html;
  updateMods();
  requestAnimationFrame(sendRegions);
}

function updateMods() {
  kbEl.querySelectorAll('[data-code="shift"]').forEach(el => {
    el.classList.toggle('armed', S.mods.shift && !S.caps);
    el.classList.toggle('locked', S.caps);
  });
  kbEl.querySelectorAll('[data-code="ctrl"]').forEach(el => el.classList.toggle('armed', S.mods.ctrl));
  kbEl.querySelectorAll('[data-code="alt"]').forEach(el => el.classList.toggle('armed', S.mods.alt));
  kbEl.querySelectorAll('[data-code="capslock"]').forEach(el => el.classList.toggle('locked', S.caps));
  // 上档符号键: shift/caps 激活时高亮键帽上的上档字符
  kbEl.classList.toggle('shift-on', S.mods.shift || S.caps);
}

// 输入法短标签变化时整树重渲染 (局部 textContent 更新在无合成器环境会触发
// 局部重绘残影, 整树重绘代价可忽略且触发频率低)
function updateImeBtn() {
  render();
}

// ---- 按键气泡: 手指遮挡键帽时, 在按键上方放大显示所按内容 ----
const bubbleMap = new Map(); // 按键元素 -> 气泡元素 (多点触控时每键一个)

function bubbleText(el) {
  if (el.dataset.type === 'start') return '开始';
  if (el.dataset.code === 'space') return '空格';
  const main = el.querySelector('.main');
  const supEl = el.querySelector('.sup');
  if (supEl && kbEl.classList.contains('shift-on')) {
    const v = supEl.textContent.trim();
    if (v) return v;
  }
  return (main && main.textContent.trim()) || el.textContent.trim();
}

function showBubble(el) {
  if (!S.bubble) return; // 设置里可关闭按键气泡 (默认开)
  if (el.dataset.type === 'act') return; // 顶栏按钮有文字标签, 不需要气泡
  const text = bubbleText(el);
  if (!text) return;
  let b = bubbleMap.get(el);
  if (!b) {
    b = document.createElement('div');
    b.className = 'kb-bubble';
    appEl.appendChild(b); // 挂在 #app 下: 不随键区整树重绘, 不并入键盘透明度
    bubbleMap.set(el, b);
  }
  b.classList.remove('fade');
  b.textContent = text;
  b.classList.toggle('small', [...text].length > 2);
  // 键上方居中, 水平夹进窗口; 顶行键的气泡允许盖住顶栏, 但不能伸出窗外
  const kr = el.getBoundingClientRect();
  const cx = Math.min(Math.max(kr.left + kr.width / 2, 10), innerWidth - 10);
  b.style.left = cx + 'px';
  b.style.top = Math.max(kr.top - 4, 2) + 'px';
  const br = b.getBoundingClientRect();
  if (br.top < 2) b.style.top = (parseFloat(b.style.top) + (2 - br.top)) + 'px';
}

function hideBubbles() {
  bubbleMap.forEach(b => {
    b.classList.add('fade');
    setTimeout(() => b.remove(), 160);
  });
  bubbleMap.clear();
}

// ---- 按键交互: 按下即出字, 退格/方向键支持长按连发 ----
let pressTimer = null, repeatTimer = null;

function pressAction(el) {
  el.classList.add('pressing');
  showBubble(el);
  act(el);
  if (el.dataset.repeat) {
    pressTimer = setTimeout(() => {
      repeatTimer = setInterval(() => act(el), 55);
    }, 420);
  }
}

// pointer/touch 去重: 部分环境同一次接触会双派发 (两事件间隔仅几毫秒)。
// 用「同一元素 + 100ms 窗口」判定; 不能用长窗口节流, 否则快速连打时
// 第二个键会被当成双派发而吞掉 (已踩坑: 500ms 节流导致连打丢键)。
let lastTap = { el: null, t: 0 };

function tapFilter(el) {
  const now = Date.now();
  if (el === lastTap.el && now - lastTap.t < 100) return;
  lastTap = { el, t: now };
  pressAction(el);
}

// ---- 拖动把手: JS 只上报按下/抬起, 跟随由 Rust 线程用全局光标坐标完成 ----
let gripHeld = false, lastGripDown = 0;

function gripDown(e) {
  e.preventDefault();
  if (Date.now() - lastGripDown < 500) return; // pointer/touch 事件去重
  lastGripDown = Date.now();
  gripHeld = true;
  kbEl.querySelectorAll('.grip').forEach(g => g.classList.add('pressing'));
  invoke('start_kb_drag').catch(() => {});
}

function gripUp() {
  if (!gripHeld) return;
  gripHeld = false;
  kbEl.querySelectorAll('.grip').forEach(g => g.classList.remove('pressing'));
  invoke('end_kb_drag').catch(() => {});
}

kbEl.addEventListener('pointerdown', e => {
  if (e.target.closest('.grip')) { gripDown(e); return; }
  const el = e.target.closest('.key');
  if (!el) return;
  e.preventDefault();
  tapFilter(el);
});

// WebKitGTK 对真实触摸只派发 touch 事件 (无 pointer 事件), 必须单独监听
kbEl.addEventListener('touchstart', e => {
  if (e.target.closest('.grip')) { gripDown(e); return; }
  const el = e.target.closest('.key');
  if (!el) return;
  e.preventDefault();
  tapFilter(el);
}, { passive: false });

function cornerAct(a) {
  if (a === 'close') invoke('minimize_to_ball');
  else if (a === 'mode') setSettings({ mode: S.mode === 'full' ? 'split' : 'full' });
  else if (a === 'float') setSettings({ float_mode: true });
  else if (a === 'dock') setSettings({ float_mode: false, kb_pos: null });
  else if (a === 'settings') invoke('toggle_settings').catch(err => showBanner(String(err)));
}

function act(el) {
  const type = el.dataset.type;
  const code = el.dataset.code;
  if (type === 'act') { cornerAct(el.dataset.act); return; }
  if (type === 'shift') {
    const now = Date.now();
    if (now - lastShiftTap < 350) { S.caps = !S.caps; S.mods.shift = false; }
    else { S.mods.shift = !S.mods.shift; }
    lastShiftTap = now;
    updateMods();
    return;
  }
  if (type === 'mod') { S.mods[code] = !S.mods[code]; updateMods(); return; }
  if (type === 'caps') { S.caps = !S.caps; updateMods(); return; }
  if (type === 'ime') {
    // 输入法切换: 发系统输入源切换快捷键 Super+Space (GNOME/IBus 默认)
    invoke('tap_key', { code: 'space', shift: false, ctrl: false, alt: false, meta: true })
      .catch(err => showBanner(String(err)));
    return;
  }
  if (type === 'start') {
    // 开始按钮: 单发 Super 键, GNOME 弹出活动概览 (等同 Windows 键)
    invoke('tap_key', { code: 'super', shift: false, ctrl: false, alt: false, meta: false })
      .catch(err => showBanner(String(err)));
    return;
  }

  const shift = el.dataset.shift === '1' || S.mods.shift || S.caps;
  invoke('tap_key', { code, shift, ctrl: S.mods.ctrl, alt: S.mods.alt, meta: false })
    .catch(err => showBanner(String(err)));
  S.mods = { shift: false, ctrl: false, alt: false };
  updateMods();
}

const endPress = () => {
  kbEl.querySelectorAll('.pressing').forEach(el => el.classList.remove('pressing'));
  clearTimeout(pressTimer);
  clearInterval(repeatTimer);
  hideBubbles();
  gripUp(); // 把手拖动可能在窗口移动后丢失元素级 up 事件, 全局兜底结束
};
window.addEventListener('pointerup', endPress);
window.addEventListener('pointercancel', endPress);
window.addEventListener('touchend', endPress);
window.addEventListener('touchcancel', endPress);
document.addEventListener('contextmenu', e => e.preventDefault());

// ---- 设置修改 (顶栏快捷键; 全部设置项在独立设置窗口里) ----
async function setSettings(patch) {
  Object.assign(S, patch);
  save();
  try { await invoke('apply_settings', { settings: cleanSettings() }); } catch (e) { showBanner(String(e)); }
  render();
}

function showBanner(msg) {
  const b = document.getElementById('banner');
  b.textContent = msg;
  b.classList.remove('hidden');
  setTimeout(() => b.classList.add('hidden'), 6000);
}

// ---- 点击穿透区域上报 (CSS 像素) ----
function sendRegions() {
  const rects = [];
  // 顶栏/工具条/键区面板整体可交互 (顶栏空白处为拖动把手)
  kbEl.querySelectorAll('.panel, .titlebar, .toolbar').forEach(el => {
    const r = el.getBoundingClientRect();
    rects.push(r.left, r.top, r.width, r.height);
  });
  invoke('update_hit_regions', { win: 'keyboard', rects }).catch(() => {});
}

new ResizeObserver(sendRegions).observe(kbEl);
window.addEventListener('resize', () => {
  appEl.style.setProperty('--kbh', innerHeight + 'px');
  appEl.style.setProperty('--kbw', innerWidth + 'px');
  sendRegions();
});

// ---- 初始化 ----
async function init() {
  appEl.style.setProperty('--kbh', innerHeight + 'px');
  appEl.style.setProperty('--kbw', innerWidth + 'px');
  try { screen = await invoke('get_screen_info'); } catch (e) { /* 用默认值 */ }
  render();
  applyTheme();

  // 把本地保存的设置推送给后端 (窗口几何按用户设置适配)
  invoke('apply_settings', { settings: cleanSettings() }).catch(() => {});

  // 与后端状态合并 (托盘菜单可能改过布局/悬浮模式等), 防止两边脱节
  invoke('get_settings').then(b => {
    if (!b) return;
    let dirty = false;
    SETTING_KEYS.forEach(k2 => {
      if (b[k2] !== undefined && b[k2] !== null && JSON.stringify(S[k2]) !== JSON.stringify(b[k2])) {
        S[k2] = b[k2];
        dirty = true;
      }
    });
    if (dirty) { save(); applyTheme(); render(); }
  }).catch(() => {});

  invoke('input_backend_name')
    .then(n => {
      if (n === 'none') showBanner('⚠ 输入后端不可用: 请按 README 将用户加入 input 组 / 安装 udev 规则, 或在 X11 会话使用');
    })
    .catch(() => {});

  invoke('set_click_through', { enabled: S.click_through }).catch(() => {});

  loadStartIcon();

  invoke('get_prefers_dark')
    .then(d => { prefersDark = !!d; applyTheme(); })
    .catch(() => {});

  await listen('layout-changed', e => {
    screen = e.payload;
    sendRegions();
  });
  await listen('ime-changed', e => {
    S.ime_label = e.payload || '中';
    updateImeBtn();
  });
  invoke('get_ime_label')
    .then(l => { S.ime_label = l; updateImeBtn(); })
    .catch(() => {});
  // 设置窗口 / 托盘改设置后, 后端回显完整设置 (含解析后的 kb_pos)
  await listen('settings-changed', e => {
    const p = e.payload || {};
    let dirty = false;
    SETTING_KEYS.forEach(k2 => {
      if (p[k2] !== undefined && JSON.stringify(S[k2]) !== JSON.stringify(p[k2])) {
        S[k2] = p[k2];
        dirty = true;
      }
    });
    if (!dirty) return;
    save();
    applyTheme();
    render();
  });
  // 主题 = 跟随系统: 后端轮询系统深浅色变化
  await listen('prefers-dark-changed', e => {
    prefersDark = !!e.payload;
    applyTheme();
  });
  // 新版本检测: ⚙ 加红点角标 (详情在设置窗口「关于」区, 打开即见更新卡片)
  await listen('update-available', () => {
    updAvailable = true;
    document.querySelectorAll('.key.tbtn[data-act="settings"]')
      .forEach(el => el.classList.add('upd'));
  });
}

init();
