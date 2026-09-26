/* 悬浮球:
   - rust 模式 (X11/XWayland 可用): JS 只上报按下/抬起, 拖动跟随由 Rust 用全局
     光标坐标完成 (鼠标与触摸一致, 无坐标反馈回路 => 不闪不抖, 触摸可拖动);
   - js 模式 (无 X11 的兜底): JS 用窗口内坐标拖动。 */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { PhysicalPosition } = window.__TAURI__.window;
const win = window.__TAURI__.window.getCurrentWindow();

const BALL_SIZE = 56;
const ball = document.getElementById('ball');
let screen = null;
let drag = null;

// 球体锚定角: 贴右/下边缘时锚定窗口右/下角, 让窗口整体留在屏幕内 (Rust 端 snap 后通知)
function setCorner(p = {}) {
  if (Array.isArray(p)) p = { h_right: p[0], v_bottom: p[1] }; // Rust 元组负载
  document.body.classList.toggle('c-h-r', !!p.h_right);
  document.body.classList.toggle('c-v-b', !!p.v_bottom);
  sendRegions();
}
listen('ball-corner', e => setCorner(e.payload || {})).catch(() => {});

async function getScreen() {
  if (!screen) {
    try { screen = await invoke('get_screen_info'); } catch (e) { /* 忽略 */ }
  }
  return screen;
}

(async () => {
  const saved = JSON.parse(localStorage.getItem('tk-ball-pos') || 'null');
  if (saved) {
    try {
      await win.setPosition(new PhysicalPosition(saved.x, saved.y));
      invoke('ball_moved', { x: saved.x, y: saved.y }).catch(() => {});
      // 依据保存位置推断球体锚定角 (右/下边缘 -> 右/下角)
      const info = await getScreen();
      if (info) {
        const sf = info.scale_factor || 1;
        const [wx, wy, ww, wh] = info.workarea || [0, 0, info.logical_width, info.logical_height];
        setCorner({
          h_right: saved.x / sf + 200 >= wx + ww - 7,
          v_bottom: saved.y / sf + 200 >= wy + wh - 7,
        });
      }
    } catch (e) { /* 忽略 */ }
  }
  setupDrag(await invoke('ball_input_mode').catch(() => 'js'));
})();

function setupDrag(mode) {
  if (mode === 'rust') {
    let lastDown = 0;
    const down = e => {
      e.preventDefault();
      try { if (e.pointerId !== undefined) ball.setPointerCapture(e.pointerId); } catch (err) { /* 忽略 */ }
      if (Date.now() - lastDown < 500) return; // pointer/touch 去重
      lastDown = Date.now();
      ball.classList.add('pressing');
      invoke('start_ball_drag').catch(() => {});
    };
    const up = () => {
      if (!ball.classList.contains('pressing')) return;
      ball.classList.remove('pressing');
      invoke('end_ball_drag').catch(() => {});
    };
    ball.addEventListener('pointerdown', down);
    // WebKitGTK 对真实触摸只派发 touch 事件, 单独监听
    ball.addEventListener('touchstart', down, { passive: false });
    ball.addEventListener('pointerup', up);
    ball.addEventListener('pointercancel', up);
    ball.addEventListener('touchend', up);
    ball.addEventListener('touchcancel', up);
  } else {
    // JS 兜底拖动 (窗口内坐标)
    ball.addEventListener('pointerdown', async e => {
      e.preventDefault();
      try { ball.setPointerCapture(e.pointerId); } catch (err) { /* 忽略 */ }
      ball.classList.add('pressing');
      let px = null, py = null;
      try { const p = await win.outerPosition(); px = p.x; py = p.y; } catch (err) { /* 忽略 */ }
      drag = { sx: e.clientX, sy: e.clientY, px, py, moved: false };
    });
    ball.addEventListener('pointermove', e => {
      if (!drag) return;
      const dx = e.clientX - drag.sx;
      const dy = e.clientY - drag.sy;
      if (Math.abs(dx) + Math.abs(dy) > 6) drag.moved = true;
      if (drag.moved && drag.px !== null) {
        const dpr = window.devicePixelRatio || 1;
        const x = Math.round(drag.px + dx * dpr);
        const y = Math.round(drag.py + dy * dpr);
        win.setPosition(new PhysicalPosition(x, y)).catch(() => {});
      }
    });
    ball.addEventListener('pointerup', async () => {
      ball.classList.remove('pressing');
      const d = drag;
      drag = null;
      if (!d) return;
      if (!d.moved) {
        invoke('restore_keyboard').catch(err => console.error(err));
      } else {
        snapToEdge();
      }
    });
    ball.addEventListener('pointercancel', () => {
      ball.classList.remove('pressing');
      const d = drag;
      drag = null;
      if (d && d.moved) snapToEdge();
    });
  }
}

// 吸附到最近的左右边缘 (优先使用工作区, 避让 dock)。
// 贴右缘时球体锚定窗口右上角、窗口整体留在屏幕内, 否则会被 WM 拉回导致贴不了边。
async function snapToEdge() {
  const info = await getScreen();
  const pos = await win.outerPosition();
  const sf = (info && info.scale_factor) || window.devicePixelRatio || 1;
  let wx = 0, wy = 0, ww = ((info && info.logical_width) || 1920);
  let wh = ((info && info.logical_height) || 1080);
  if (info && info.workarea) {
    [wx, wy, ww, wh] = info.workarea;
  }
  // GTK 强制球窗口最小 200x200
  const outer = await win.outerSize().catch(() => null);
  const wwin = outer ? outer.width / sf : 200;
  const hwin = outer ? outer.height / sf : 200;
  const size = BALL_SIZE;
  const margin = 6;
  // 当前逻辑全局坐标
  const plx = pos.x / sf;
  const ply = pos.y / sf;
  const toLeft = plx + size / 2 < wx + ww / 2;
  const tx = toLeft ? wx + margin : wx + ww - margin - wwin;
  let tyv = Math.min(Math.max(ply, wy + margin), wy + wh - size - margin);
  const vBottom = tyv + hwin > wy + wh - margin + 1;
  const ty = vBottom ? tyv + size - hwin : tyv;
  setCorner({ h_right: !toLeft, v_bottom: vBottom });
  const steps = 8;
  let i = 0;
  const tick = () => {
    i++;
    const x = Math.round(pos.x + (tx * sf - pos.x) * (i / steps));
    const y = Math.round(pos.y + (ty * sf - pos.y) * (i / steps));
    win.setPosition(new PhysicalPosition(x, y)).catch(() => {});
    if (i < steps) {
      requestAnimationFrame(tick);
    } else {
      localStorage.setItem('tk-ball-pos', JSON.stringify({ x: tx * sf, y: ty * sf }));
      invoke('ball_moved', { x: tx * sf, y: ty * sf }).catch(() => {});
    }
  };
  tick();
}

// 上报球体可交互区域 (窗口其余部分点击穿透; 区域相对窗口原点, 不随窗口移动变化)。
// 用锚点+固定尺寸推算而非 getBoundingClientRect: 元素带 transform 过渡,
// 过渡期间读 rect 会拿到缩放中间值; 外扩 6px 兜住指尖按在球体边缘的情况。
function sendRegions() {
  const hRight = document.body.classList.contains('c-h-r');
  const vBottom = document.body.classList.contains('c-v-b');
  const pad = 6;
  const x = Math.max(0, (hRight ? innerWidth - BALL_SIZE : 0) - pad);
  const y = Math.max(0, (vBottom ? innerHeight - BALL_SIZE : 0) - pad);
  const w = Math.min(innerWidth - x, BALL_SIZE + pad * 2);
  const h = Math.min(innerHeight - y, BALL_SIZE + pad * 2);
  invoke('update_hit_regions', { win: 'ball', rects: [x, y, w, h] }).catch(() => {});
}
sendRegions();
window.addEventListener('resize', sendRegions);

document.addEventListener('contextmenu', e => e.preventDefault());
