/* 独立设置窗口: 设置面板从这里移出, 不再与键盘共用窗体。
   数据流: 本窗口与键盘页共享 localStorage (键 tk-settings-v1), 修改后经
   `apply_settings` 推给后端, 后端 refit 后 emit `settings-changed` 回显,
   两边窗口都据回显合并, 保持一致。
   本窗口与键盘一样不抢焦点 (后端 disable_focus + WM_HINTS),
   调整设置时目标应用焦点不变, 键盘可正常打字。 */
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
S.split_half_units = clampSplitUnits(S.split_half_units);
S.tp_speed = clampTpSpeed(S.tp_speed);
let prefersDark = false;

function clampSplitUnits(v) {
  const n = Number(v);
  return Number.isFinite(n) ? Math.min(9.5, Math.max(5.0, n)) : 6.8;
}

function clampTpSpeed(v) {
  const n = Number(v);
  return Number.isFinite(n) ? Math.min(3, Math.max(0.3, n)) : 1.0;
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

function cleanSettings() {
  const out = {};
  SETTING_KEYS.forEach(k => { out[k] = S[k]; });
  return out;
}

// ---- 主题 ----
function applyTheme() {
  const dark = S.theme === 'dark' || (S.theme === 'auto' && prefersDark);
  document.documentElement.classList.toggle('dark', dark);
}

// ---- 控件回显 ----
function updateUI() {
  const op = document.getElementById('opacity');
  const sz = document.getElementById('size');
  const fw = document.getElementById('floatw');
  const sw = document.getElementById('splitw');
  if (document.activeElement !== op) op.value = Math.round(S.opacity * 100);
  if (document.activeElement !== sz) sz.value = Math.round(S.kb_scale * 100);
  document.getElementById('opacity-val').textContent = Math.round(S.opacity * 100) + '%';
  document.getElementById('size-val').textContent = Math.round(S.kb_scale * 100) + '%';
  document.getElementById('mode-full').classList.toggle('active', S.mode === 'full');
  document.getElementById('mode-split').classList.toggle('active', S.mode === 'split');
  document.getElementById('win-dock').classList.toggle('active', !S.float_mode);
  document.getElementById('win-float').classList.toggle('active', S.float_mode);
  if (document.activeElement !== fw) fw.value = Math.round((S.float_width || 0.6) * 100);
  document.getElementById('floatw-val').textContent = Math.round((S.float_width || 0.6) * 100) + '%';
  document.getElementById('floatw-item').classList.toggle('hidden', !S.float_mode);
  if (document.activeElement !== sw) sw.value = Math.round(clampSplitUnits(S.split_half_units) * 10);
  document.getElementById('splitw-val').textContent = clampSplitUnits(S.split_half_units).toFixed(1);
  document.getElementById('splitw-item').classList.toggle('hidden', S.mode !== 'split');
  document.getElementById('numrow').checked = S.num_row;
  document.getElementById('numrow-item').classList.toggle('hidden', S.mode === 'full');
  document.getElementById('clickthrough').checked = S.click_through;
  document.getElementById('bubble').checked = !!S.bubble;
  const tps = document.getElementById('tpspeed');
  if (document.activeElement !== tps) tps.value = Math.round(clampTpSpeed(S.tp_speed) * 100);
  document.getElementById('tpspeed-val').textContent = Math.round(clampTpSpeed(S.tp_speed) * 100) + '%';
  document.getElementById('tp-tap').checked = S.tp_tap_click !== false;
  document.getElementById('tp-natural').checked = S.tp_natural_scroll !== false;
  ['light', 'dark', 'auto'].forEach(t => {
    document.getElementById('theme-' + t).classList.toggle('active', S.theme === t);
  });
}

async function setSettings(patch) {
  Object.assign(S, patch);
  save();
  try { await invoke('apply_settings', { settings: cleanSettings() }); } catch (e) { console.error(e); }
  updateUI();
  applyTheme();
}

// ---- 控件绑定 ----
document.getElementById('opacity').addEventListener('input', e => setSettings({ opacity: e.target.value / 100 }));
document.getElementById('size').addEventListener('input', e => setSettings({ kb_scale: e.target.value / 100 }));
// 分段按钮/关闭按钮: 按下即触发 (pointer/touch 双监听 + 100ms 去重, 同键盘按键模式),
// 不依赖 WebKit 的 touch→click 合成, 触摸响应也更跟手
function onTap(el, fn) {
  let last = 0;
  const h = e => {
    e.preventDefault();
    const now = Date.now();
    if (now - last < 100) return; // pointer/touch 双派发去重
    last = now;
    fn();
  };
  el.addEventListener('pointerdown', h);
  el.addEventListener('touchstart', h, { passive: false });
}
onTap(document.getElementById('mode-full'), () => setSettings({ mode: 'full' }));
onTap(document.getElementById('mode-split'), () => setSettings({ mode: 'split' }));
onTap(document.getElementById('win-dock'), () => setSettings({ float_mode: false, kb_pos: null }));
onTap(document.getElementById('win-float'), () => setSettings({ float_mode: true }));
['light', 'dark', 'auto'].forEach(t => {
  onTap(document.getElementById('theme-' + t), () => setSettings({ theme: t }));
});
onTap(document.getElementById('set-close'), () => invoke('hide_settings').catch(() => {}));
document.getElementById('floatw').addEventListener('input', e => setSettings({ float_width: e.target.value / 100 }));
// 分裂宽度: 键距数 5.0 - 9.5 (滑块值 ×0.1)
document.getElementById('splitw').addEventListener('input', e => setSettings({ split_half_units: clampSplitUnits(e.target.value / 10) }));
document.getElementById('numrow').addEventListener('change', e => setSettings({ num_row: e.target.checked }));
document.getElementById('clickthrough').addEventListener('change', e => {
  setSettings({ click_through: e.target.checked });
  invoke('set_click_through', { enabled: e.target.checked }).catch(() => {});
});
document.getElementById('bubble').addEventListener('change', e => setSettings({ bubble: e.target.checked }));
// 触摸板速度: 30% - 300% (滑块值 /100)
document.getElementById('tpspeed').addEventListener('input', e => setSettings({ tp_speed: clampTpSpeed(e.target.value / 100) }));
document.getElementById('tp-tap').addEventListener('change', e => setSettings({ tp_tap_click: e.target.checked }));
document.getElementById('tp-natural').addEventListener('change', e => setSettings({ tp_natural_scroll: e.target.checked }));

// 标题栏空白处拖动窗口 (设置窗口无系统装饰; pointer/touch 双监听 + 去重)
let lastHeadDown = 0;
function headDown(e) {
  if (e.target.closest('#set-close')) return;
  e.preventDefault();
  if (Date.now() - lastHeadDown < 500) return; // pointer/touch 去重
  lastHeadDown = Date.now();
  win.startDragging().catch(() => {});
}
document.getElementById('set-head').addEventListener('pointerdown', headDown);
document.getElementById('set-head').addEventListener('touchstart', headDown, { passive: false });
document.addEventListener('contextmenu', e => e.preventDefault());

// ---- 关于 / 新版本检测 (后端 updater.rs: 启动 8s 后自动查, 每天一次) ----
// 自动检查失败静默; 手动点「检查更新」才提示失败原因。
// 发现新版本 → 后端 emit `update-available` → 显示卡片, 「查看更新」
// 经 open_url 命令用系统浏览器打开 Release 页面手动下载 (Linux 无安全自动安装)。
let updateUrl = '';
function showUpdate(p) {
  updateUrl = p.releaseUrl || '';
  document.getElementById('update-version').textContent = 'v' + p.newVersion;
  const notes = document.getElementById('update-notes');
  notes.textContent = p.notes || '';
  notes.classList.toggle('hidden', !p.notes);
  document.getElementById('update-card').classList.remove('hidden');
}
onTap(document.getElementById('check-update'), async () => {
  const result = document.getElementById('update-result');
  result.textContent = '正在检查…';
  try {
    const r = await invoke('check_update');
    if (r.kind === 'upToDate') result.textContent = '已是最新版本 v' + r.current;
    else if (r.kind === 'available') result.textContent = '发现新版本 v' + r.newVersion;
    else result.textContent = '检查失败: ' + (r.message || '网络异常');
  } catch (e) {
    result.textContent = '检查失败: ' + e;
  }
});
onTap(document.getElementById('update-open'), () => {
  if (updateUrl) invoke('open_url', { url: updateUrl }).catch(() => {});
});

// ---- 初始化 ----
(async () => {
  updateUI();
  applyTheme();
  // 与后端状态合并: 后端值与默认值不同说明有真实状态 (托盘/键盘页改过), 以后端为准;
  // 相同则说明后端尚未收到推送 (启动竞态), 保留本地持久化值, 之后靠 settings-changed 对齐
  invoke('get_settings').then(b => {
    if (!b) return;
    let dirty = false;
    SETTING_KEYS.forEach(k => {
      if (b[k] !== undefined && b[k] !== null &&
          JSON.stringify(b[k]) !== JSON.stringify(DEFAULTS[k]) &&
          JSON.stringify(S[k]) !== JSON.stringify(b[k])) {
        S[k] = b[k];
        dirty = true;
      }
    });
    if (dirty) { save(); updateUI(); applyTheme(); }
  }).catch(() => {});

  invoke('get_prefers_dark')
    .then(d => { prefersDark = !!d; applyTheme(); })
    .catch(() => {});

  // 当前版本号 (来源 tauri.conf.json) + 后台检查发现的新版本
  invoke('app_version')
    .then(v => { document.getElementById('about-version').textContent = '版本 v' + v; })
    .catch(() => {});
  await listen('update-available', e => {
    if (e.payload) showUpdate(e.payload);
  });

  await listen('prefers-dark-changed', e => {
    prefersDark = !!e.payload;
    applyTheme();
  });
  // 键盘页顶栏/托盘改设置后的回显: 合并并刷新控件 (不再回推, 避免循环)
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
    save();
    updateUI();
    applyTheme();
  });
})();
