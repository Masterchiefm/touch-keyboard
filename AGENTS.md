# AGENTS.md — touch-keyboard(Linux 触屏键盘)

给 AI 编码代理的项目上下文与操作手册。改动代码前请通读「关键机制」与「已知的坑」。

## 项目概述

基于 **Tauri 2 + Rust** 的 Linux 触摸屏虚拟键盘。运行于 XWayland(X11 模式)或纯 X11,
按键注入支持 **XTEST** 与 **uinput** 双后端(X11 与 Wayland 会话通吃)。
功能:完整/分裂布局(分裂宽度可调)、屏幕自适应、dock 避让、透明度/大小调节、悬浮球、
托盘常驻、空白穿透、独立设置窗口、日夜主题(可跟随系统)、「开始」键系统图标、按键气泡、
**屏幕触摸板**(类 Windows 系统触摸板,独立窗口 + 托盘开关)。

目标环境:GNOME Wayland(Ubuntu 22.04)+ XWayland;需兼容纯 X11 会话。

## 常用命令

```bash
# 所有 cargo 命令在 src-tauri/ 下执行(根目录无 Cargo.toml)
cd src-tauri
cargo build                 # 调试构建
cargo build --release       # 发布构建(~3分钟, LTO)

# 运行(前端为 ui/ 静态文件, 无 Node; UI 改动需重新编译才生效, 资源编译期嵌入)
./target/release/touch-keyboard

# 打包 deb/AppImage(需要 tauri-cli, 本机未装)
cargo install tauri-cli --version "^2" && cargo tauri build

# 重新生成应用图标(纯标准库 Python)
python3 tools/gen_icons.py

# uinput 权限配置(Wayland 会话按键注入必需; 本机已配置)
sudo bash scripts/setup-uinput.sh

# 发版: 同步版本号(tauri.conf.json + Cargo.toml) → 提交 → 打 v* 标签 → 推送后
# CI(.github/workflows/build.yml) 自动构建 .deb/.AppImage 并发布 GitHub Release
scripts/release.sh 0.2.0 && git push origin main --tags
```

环境变量:
- `TOUCH_KB_BACKEND=x11|uinput` 强制按键/鼠标注入后端(默认自动:Wayland 会话优先 uinput)
- `TOUCH_KB_WORKAREA=x,y,w,h` 手动指定工作区物理像素(WM 不维护 _NET_WORKAREA 时的 dock 避让)
- `TOUCH_KB_TP_SHOW=1` 启动即显示触摸板窗口(调试/自启动)
- `TOUCH_KB_DEBUG=1` 触摸板注入与手势线程的调试日志(stderr)

## 代码地图

```
ui/                      # 前端, 纯静态 HTML/JS, 使用 window.__TAURI__ 全局 API
  main.js                # 键盘: 布局渲染/按键/一次性修饰键/拖动把手/按键气泡/穿透区域上报/
                         # ⚙ 更新角标(update-available 事件, 状态并进 render 的 HTML)
  ball.js                # 悬浮球: pointer+touch 双监听, 拖动上报, 球体区域上报
  touchpad.html+js       # 屏幕触摸板窗口: 表面 + 左右按键; 触点数上报(tp_surface)、touchmove
                         # 驱动(可用时)、轻触/左右键、顶栏拖动; 手势主体见 Rust tp_motion_poll
  settings.html+js       # 独立设置窗口: 全部设置项(键盘+触摸板)+主题分段选择+「关于/更新」区
                         # (版本号显示/检查更新按钮/更新卡片, 固定面板最底部); 与键盘页共享
                         # localStorage, 经 apply_settings / settings-changed 双向同步
  tux.svg                # 「开始」键兜底图标: Linux 企鹅 (取自 genepad.cn, fill=currentColor 随主题变色)
  style.css              # 触摸友好样式; html.dark 深色主题变量; #settings/#tp 必须不透明(勿并入键盘透明度)
src-tauri/src/
  main.rs                # 窗口几何(停靠/悬浮)/托盘/命令/球与键盘拖动线程/XShape 应用/
                         # 设置窗口显隐定位 (toggle_settings)/系统深浅色轮询 (theme_poll)/
                         # 「开始」键系统图标查找 (freedesktop start-here → data URL)/
                         # 触摸板窗口显隐定位 + 鼠标注入命令 + 归位点/手势轮询线程
  input/mod.rs           # 后端选择: Wayland→uinput优先, X11→XTEST优先; TOUCH_KB_BACKEND 可覆盖
  input/mouse.rs         # 触摸板鼠标注入抽象(move_rel/button/scroll/warp)+ XTEST 实现 + 后端选择
  input/x11.rs           # XTEST 按键注入(读服务器 keysym→keycode 键表, 支持 Shift/AltGr)
  input/uinput.rs        # 内核虚拟键盘(假定美式 QWERTY 布局) + 虚拟鼠标(相对+三键+滚轮)
                         # + 画笔悬停设备(绝对定位 warp 用)
  xutil.rs               # X 连接: WM_HINTS 不抢焦点/XShape 输入区域/_NET_WORKAREA/光标坐标/
                         # XTEST 绝对移动 (warp_pointer)
  updater.rs             # 新版本检测(纯函数+ureq): GitHub releases/latest 查询、版本三段式
                         # 解析与比较(含单测); 自动路径失败静默, 只检测不自动安装
examples/set_shape.rs    # 调试: 查询/设置窗口输入区域 (cargo run --example set_shape -- <id> query)
examples/set_workarea.rs # 调试: 设置 root 的 _NET_WORKAREA
scripts/setup-uinput.sh  # udev 规则 + input 组
scripts/focus-watch.sh   # 轮询 _NET_ACTIVE_WINDOW 观察焦点(仅对 X11 目标可见)
scripts/touch-tap.py     # uinput 虚拟触摸屏注入真实触摸(需 input 组; 只发 ABS_X/Y+BTN_TOUCH,
                         #  mutter 会当数位板处理, 不能完全等同真触摸屏)
tools/gen_icons.py       # 图标生成
```

## 关键机制(修改前必读)

1. **不抢焦点 = GTK accept_focus + 每次绘制后重新断言**。
   tao 在窗口以 `focus:false` 创建后, 会在**首次绘制时把 accept_focus 改回 true**
   (tao window.rs 的 connect_draw 恢复逻辑)。`disable_focus()` 里的
   `gw.connect_draw(...)` 每帧重设 `set_accept_focus(false)` 来中和它, **不可删除**。
   焦点被抢的症状 = 触摸键盘后目标输入框失焦(尤其 Wayland 原生应用)。

2. **点击穿透 = XShape 输入区域(服务端静态生效), 不是轮询**。
   前端把可交互区域(CSS 像素)经 `update_hit_regions` 上报, 后端换算物理像素后
   用 `shape_rectangles(SK::INPUT)` 设置。区域内事件进窗口, 区域外穿透,
   真实触摸"瞬移落下"也无延迟无竞争。**不要退回轮询 `set_ignore_cursor_events` 的方案**
   —— 轮询有 25ms 竞态, 触摸穿透丢焦点(已踩坑)。`xwininfo -shape` 不显示 INPUT shape,
   验证要用 `examples/set_shape.rs <id> query`。

3. **触摸事件双监听**:WebKitGTK 对真实触摸只派发 `touchstart/touchend`, 不一定派发
   `pointerdown`。按键与悬浮球必须 pointer + touch 双监听。去重用「同一元素 + 100ms 窗口」
   (`tapFilter`): 同一次接触双派发的事件间隔仅几毫秒, 而人手连打同键不可能快于 100ms。
   **不能用长窗口节流**(如 500ms 内忽略): 快速连打的第二个键会被吞掉 (已踩坑)。

4. **悬浮球拖动 = Rust 线程全局光标轮询**(`ball_drag_poll`)。JS 只上报按下/抬起。
   窗口位置 = 起始窗口位置 + (当前光标 - 起始光标), 与窗口自身移动解耦。
   若用窗口内坐标(clientX)拖动: 鼠标闪烁、触摸完全拖不动(反馈回路, 已踩坑)。
   拖动结束(吸附/恢复)必须 `run_on_main_thread` —— GTK 的 X 连接非线程安全,
   后台线程调 `current_monitor()` 等 GTK 查询会触发 xcb abort(SIGABRT)。

5. **Wayland/X11 策略**:main() 里 DISPLAY 存在即强制 `GDK_BACKEND=x11`(须覆盖
   会话预置的 GDK_BACKEND=wayland), 以获得定位/置顶/穿透能力(Wayland 原生窗口没有)。
   按键注入独立于窗口后端: Wayland 会话用 uinput 才能到达原生 Wayland 应用。

6. **dock 避让**:refit 读取 `_NET_WORKAREA`(mutter/kwin 维护)裁剪显示器矩形。
   GNOME 的 dock 画在所有 X11 窗口之上, **不可能盖过 dock**, 只能避让。

7. **设置同步流向(键盘页/设置窗口/触摸板三端)**:localStorage(`tk-settings-v1`)是设置的
   源头;键盘页 init 时推送 `apply_settings`,后端 refit 后 emit `settings-changed`
   给 **keyboard、settings、touchpad 三个窗口**,各窗口都合并回显并写回 localStorage ——
   键盘页顶栏快捷键、设置窗口控件、托盘菜单三方改动因此保持一致。设置窗口 init 时用
   `get_settings` 合并后端状态,但只接受「与默认值不同」的字段(避免启动竞态把持久化
   值打回默认)。新增设置项要同时改:Settings 结构体、Settings::default、
   main.js 与 settings.js 的 DEFAULTS/save/cleanSettings(touchpad.js 同步同一份键表)、
   设置窗口 HTML 与绑定。

8. **键盘拖动与悬浮模式**(`kb_drag_poll` + `float_mode`): 顶栏 `.grip` 把手按下/抬起
   只由 JS 上报 `start_kb_drag`/`end_kb_drag`, 跟随同悬浮球(全局光标轮询, 主线程收尾)。
   拖动过(moved)松手 → `finish_kb_drag` 置 `float_mode=true` 且清空 `kb_pos`,
   refit 以**当前窗口顶边中点为锚**缩小窗口(视觉上从落点"分离"), 解析出的 kb_pos 经
   `settings-changed` 回流给前端持久化。停靠模式所有几何由 refit 决定(忽略 kb_pos);
   悬浮模式位置 = kb_pos(物理像素), 无记录才走锚点推算。改锚点/夹取逻辑只动 refit。

9. **设置窗口独立且不抢焦点**:label `settings`(settings.html), tauri.conf 声明、
   启动隐藏。与键盘一样走 `disable_focus`(GTK accept_focus)+ `relax_focus`
   (WM_HINTS input=False, `toggle_settings` 每次显示时补一次)—— 调设置时目标应用
   焦点不变, 键盘仍可打字。定位 `position_settings` 水平居中于键盘、底边贴键盘顶上方;
   **隐藏窗口的 `outer_size()` 可能返回 0**(GTK 未完成分配), 计算时要过滤并回退配置
   尺寸, 显示后再定位一次。窗口拖动用 JS `win.startDragging()`(标题栏空白处),
   ✕/`hide_settings` 只隐藏不销毁; 键盘收起(`hide_kb_show_ball`)时一并隐藏。
   布局铁律: **条件显示项(悬浮宽度/分裂宽度)必须放在面板底部** —— 它们在模式切换时
   插入/移除, 放在按钮中间会把下方控件顶得移位, 下一次点按落在原位就点到滑块,
   表现为"按钮点不了, 只有滑杆能动" (已踩坑)。分段按钮/关闭按钮用「按下即触发」
   (`onTap`: pointerdown+touchstart 双监听 + 100ms 去重, 同机制 3),
   不依赖 WebKit 的 touch→click 合成。

10. **主题(浅/深/跟随系统, 默认 auto)**:前端 `S.theme`,解析后切换 `html.dark` 类
    (两窗口各自 applyTheme,配色集中在 style.css 的 `html.dark` 变量覆盖)。
    「跟随系统」由后端 `theme_poll` 线程每 3s 查 `gsettings ... color-scheme`
    (非 GNOME/失败按浅色), 变化时 emit `prefers-dark-changed`; 初始值走
    `get_prefers_dark` 命令。主题功能上线前的旧默认「浅色」在首次运行时一次性迁移为
    auto(`tk-theme-migrated` 标记位, 之后用户手动选的浅色不再被动)。

11. **「开始」键图标三级兜底**:Rust `start_icon_url`(OnceLock 缓存)按 freedesktop
    规范查 `start-here`/`distributor-logo`(gsettings 图标主题 → index.theme Inherits
    链 → hicolor → /usr/share/pixmaps;svg 优先、尺寸大者优先)返回 data URL;
    找不到时前端 fetch 内置 `ui/tux.svg`(企鹅, fill=currentColor); 都失败回退文字
    「开始」。系统图标深色主题下用 CSS `invert+hue-rotate` 反相。

12. **按键气泡**(`showBubble`/`hideBubbles`): 按下即在与键同名的固定定位元素上方显示
    放大字符(上档键显示当前生效字符, 空格→空格, 开始→开始), pointerup 全局兜底隐藏。
    默认开启, 设置项 `bubble` 可关(`showBubble` 入口判断)。配色同键帽(`--key`/`--line`
    随主题: 浅色白底描边、深色灰底浅字), 箭头用旋转 45° 小方块 + `background: inherit` 自动跟随。
    气泡挂在 `#app` 下 —— 不能挂进 `#kb`(整树 render 会清掉), 也不并入键盘透明度;
    多点触控按按键元素各留一个; 顶栏 `.tbtn`(自带文字标签)不出气泡。

13. **屏幕触摸板**(label `touchpad`, touchpad.html, 托盘勾选菜单开关, 440×400):
    **核心约束: mutter 的触摸指针模拟会在触摸期间把合成器光标"胶"在手指位置(绝对吸附)**,
    且 **mutter 不转发 XTEST 移动、忽略 uinput 虚拟画笔(已实测)** —— 没有任何绝对定位
    执行器, 唯一可用的是 uinput 相对移动。对策是**虚拟指针 V + 阻尼伺服**:
    - 手指位移读自被胶住的光标(它就是手指位置的镜像), `V += 手指位移 × 增益`
      (增益 = 工作区宽/板宽 × `tp_speed`, 全板扫过 ≈ 一屏);
    - **轮询路径(touchmove 不可用时)手势期间零注入**(与胶合拉锯 = 闪烁, 已踩坑),
      抬手后的"回位窗口"(~500ms)由 `tp_motion_poll` 伺服把光标从板上鬼影拉回 V
      (每拍注入 (V−光标)×0.5, 几何收敛, 加速度失配也收敛);
    - **touchmove 可用时**(JS 发过 `tp_move`)手势期间伺服实时追 V(阻尼 0.4,
      有轻微跟随延迟但光标跟手); 点击/拖动按下前 `tp_servo_v` 伺服到位再按键;
    - **鬼影只在手势期间隐藏**(html.gesturing + CSS cursor:none; 平时光标正常显示,
      永久隐藏会在回位失效时光标永远不可见, 已踩坑);
    - 空闲时 `tp_cursor_poll`(30ms)让 V 跟随真实光标(真实鼠标移动后从新位置继续;
      采样落在触摸板窗口内时跳过 —— 那是鬼影)。
    手势: 单指移动 / 轻触单击 / **长按 ~350ms 再滑动 = 拖动**(单指完成, 避开指针模拟
    只跟一指)/ 双指滚动(锁定至全部抬起, 自然滚动可关)/ 双指轻触右键 / 三指轻触中键;
    轻触判定 = 时长(JS < 360ms)+ 行程(`tp_travel` ≤ 20px);
    左右按键 = 轻触完整点击(按住无意义 —— 按住期间胶合使点击落在自家窗口上)。
    **表面/左右按键只认 touch 事件, 有意忽略鼠标 pointer 事件** —— 注入的指针事件落回
    自家窗口会形成反馈回路; 顶栏拖动/✕ 接受 pointer+touch 但有 400ms 注入防误触窗口
    (guardMouse)。设置项:`tp_speed`/`tp_tap_click`/`tp_natural_scroll`/`tp_pos`
    (窗口位置, `WindowEvent::Moved` 只在可见时记录, 隐藏时随 settings-changed 回流持久化)。
    默认位置: 键盘可见时居中于键盘上方, 否则工作区中央; 显示后夹取进工作区。
    **触摸板独立于键盘显隐**(键盘收起成球时保持)。

14. **新版本检测**(updater.rs + main.rs `update_loop`, 参照 zcode-speed-panel):
    后台线程启动 8s 后查一次 `api.github.com/.../releases/latest`, 之后每小时醒一次、
    距上次成功 ≥24h 才真正发请求。版本比较用三段式解析(`v` 前缀可省, 先行/元数据后缀忽略,
    **解析失败的 tag 宁可漏报不误报**)。发现新版本 → `app.emit("update-available")` 广播:
    键盘页 ⚙ 加 `.upd` 红点(角标状态写进 render() 的 HTML, 整树重绘不丢), 设置窗口「关于」区
    显示更新卡片(条件显示项, 固定面板最底部)。「查看更新」经 `open_url` 命令(仅 https)用
    xdg-open 打开 Release 页 —— **Linux 无安全的自动安装路径(deb 需 root), 只检测不自动下载安装**。
    自动检查路径失败完全静默; 手动「检查更新」(check_update 命令)才返回失败原因。
    版本号唯一定义在 tauri.conf.json 与 Cargo.toml(两处须一致, 发布用 scripts/release.sh 同步),
    CI 推 v* 标签自动构建 .deb/.AppImage 发 Release —— 检测读的就是这个 Release。

## 已知的坑

- **悬浮球的 X 窗口首次显示时才创建**:启动时 find_window_id 找不到"悬浮球"是正常的,
  所以球的输入区域在 `hide_kb_show_ball` 的 `ball.show()` 之后按需查找并应用。
- **隐藏窗口的 outer_size() 不可信**:GTK 未显示(或未完成分配)的窗口可能返回 0x0,
  设置窗口首次定位即因此跑偏(已踩坑); 处理:过滤 0 回退配置尺寸, 显示后再定位一次。
- **GTK 窗口最小 200x200**:56px 的悬浮球窗口会被 GTK 放大到 200x200, 无法缩小。
  处理:窗口保持 200x200, 球体按锚定角画在窗口一角(`ball-corner` 事件 + `c-h-r`/`c-v-b`
  类), 贴右/下边缘时锚定右/下角, 窗口整体留在屏幕内 —— 否则 WM 会把伸出屏幕的窗口
  拉回, 球体永远差 ~144px 贴不了边。穿透区域用锚点+固定尺寸+6px 外扩推算
  (`sendRegions`), 不要用 getBoundingClientRect: 元素 transform 过渡期间会读到中间值。
- **tao panic**:窗口未 map 完成时调用 `set_ignore_cursor_events` 会在 tao 内部
  `unwrap()` 崩溃(我们已不再使用该 API, 但引入新穿透逻辑时勿用)。
- **XTEST 实验的局限**:xdotool/XTEST 注入的点击绕过 mutter 的 WM 焦点策略,
  无法用它复现/验证"真实触摸抢焦点"问题;XWayland 会把触摸同步到核心指针,
  但鼠标点击与真实触摸在 mutter 里走不同路径。
  **本机 mutter 不转发 XTEST 指针移动**(xdotool mousemove / XTEST 相对与绝对移动都只动
  XWayland 内部指针, 合成器光标不动, 已实测) —— Wayland 会话的触摸板指针移动/绝对定位
  只能走 uinput(相对鼠标), XTEST 后端仅用于真 X11 会话。
  **uinput 虚拟画笔(BTN_TOOL_PEN 悬停/接触)也不被本机 mutter 当作光标移动**(设备被
  识别为 ID_INPUT_TABLET 但光标纹丝不动, 带/不带压力轴都试过, 已实测) —— 绝对定位
  在 Wayland 会话没有可用路径, 触摸板用阻尼相对伺服逼近(见机制 13)。
- **触摸注入管线可能整体卡死**(测试踩坑): 反复创建/销毁 uinput 触摸屏设备的过程中,
  mutter 的触摸处理可能整个失效 —— 虚拟触摸屏、真实触摸屏节点直写、完整 MT 协议注入
  全部无响应(光标冻结、窗口收不到 wl_touch), 而 uinput 鼠标/键盘照常。此时只能重启
  图形会话恢复。遇到"touch-tap.py 突然全部失效"先怀疑会话状态, 别急着改应用代码。
- **uinput 后端按键按物理位置发码**, 非美式布局的 X11 会话请用 XTEST 后端。
- **GNOME 托盘**需 AppIndicator 扩展;KDE 原生支持。
- **pkill -f 模式会匹配到自身命令行**, 杀进程请用 `pkill -9 -x touch-keyboard`
  (精确进程名), 否则可能把自己的 shell 一起杀掉。
- **本机网络**:直连境外慢, 已配置镜像:crates→rsproxy、rustup→中科大、
  apt→腾讯云(sources.list 已改, 备份在 /etc/apt/sources.list.bak.touchkb)。
  备用代理 192.168.1.2:10809。

## 测试方法

- **无头冒烟**:`scripts/smoke-test.sh` 用 Xvfb :99 启动应用 + xwininfo 截图
  (import 在此环境会挂起, 截图用 `xwd -root -silent | convert xwd:- out.png`)。
  Xvfb 下需 `WEBKIT_DISABLE_COMPOSITING_MODE=1`;透明区域显示为黑色属正常(无合成器),
  重绘残留旧键帽也属伪影。Xvfb 共享本机 dconf:主题「跟随系统」会命中本机深浅色,
  测试主题分支时可用 `gsettings set org.gnome.desktop.interface color-scheme ...`
  (测完记得恢复)。测试会污染应用 localStorage, 需要干净状态时删除
  `~/.local/share/com.touchkb.keyboard/localstorage`。
- **按键注入验证**:`DISPLAY=:0 xev` 做焦点目标, `xdotool click` 点按键,
  看 xev 日志的 XLookupString;uinput 后端可用 `scripts/touch-tap.py` 注入真实触摸
  (支持 `tap X Y` / `drag X1 Y1 X2 Y2 [ms]` / `down` / `up` / `warp X Y`(画笔悬停绝对
  移动光标, 测试归位用))。触摸板调试日志: 应用加 `TOUCH_KB_DEBUG=1`(tp_move/tp_scroll/
  手势线程/归位全打 stderr)。
- **焦点验证**:`scripts/focus-watch.sh 30 /tmp/focus.log` 轮询 _NET_ACTIVE_WINDOW。
  注意它只反映 X11 窗口焦点;Wayland 原生应用(gedit 等)聚焦时恒为 0x0, 不可观测,
  最终需真机触摸验证。
- **多进程 X 状态注意**:沙箱环境下不同调用间 X 属性/窗口的可见性可能不一致,
  关键验证请在同一条命令内完成"启动+操作+观测"。

## 约定

- 代码注释与 UI 文案使用中文;UI 字体依赖 Noto Sans CJK / WenQuanYi Micro Hei。
- **UI 风格参照 Windows 屏幕键盘**:浅色(默认)面板底 #dbdcdd、字符键白、功能键灰
  #ececee、激活态强调蓝 #0067c0;深色(html.dark)面板 #26262a、键 #3a3a3f、强调蓝
  #4cc2ff。CSS 尺寸以 `--u`(= 键盘窗口高 / 5.82, 即键距)为基准单位等比缩放。
- 布局常量在 main.rs 顶部:KB_ASPECT=0.375(完整键盘高/宽比, 键帽近正方, 高度随窗口宽度
  而非屏幕高度变化)、SPLIT_HALF_UNITS=6.8(分裂键距推算的默认基准, 也是分裂宽度
  设置的默认值;窗口高度只由它推算的键距决定, 与用户的分裂宽度设置无关)、
  BALL_SIZE=56、TP_WIN_W/H=440×400(触摸板窗口默认尺寸, 定位回退用)。
- 完整布局在 main.js `fullRows()`: 固定标准 5 行 (Esc+`~`+数字行含上档符号+⌫ / Tab+qwerty+
  {[ }| \\|+Del / Caps+asdf+↵ / 双 Shift+zxcvbnm+,< .> ?/ +↑ / Ctrl Alt 开始 空格 Alt Ctrl
  ←↓→中), **各行 flex 权重合计恒为 15.55**; 首行为 0.68 倍矮行 (CSS `.row-nums`),
  **不受数字行开关影响**(开关仅作用分裂布局); 空格键帽留白(同 Windows);
  功能键(Esc/Tab/Caps/Shift/⌫/Del/Enter/方向键)灰色, **Shift 用文字标签不用箭头符号**;
  上档符号键用 `sup()` 生成双行键帽, shift/caps 激活时发上档字符 (上档符号都经数字/标点键
  输入, 不设独立死键)。**符号层(symRows)已整体移除**, 全部符号经 Shift 上档输入。
  「开始」键(data-type `start`)单发 **Super** 键(GNOME 活动概览, 等同 Windows 键,
  后端键码表新增 `super`), 键帽显示**图标**(系统 start-here → 内置企鹅 tux.svg → 文字,
  见关键机制 11); 「中」键(data-type `ime`)发 **Super+Space** 切换系统输入源
  (GNOME/IBus 默认); 分裂模式的数字/符号依赖「数字行」开关。
- **输入法标签实时同步**: Rust 端 `ime_poll` 线程每 1s 轮询 `ibus engine`(失败回退
  `fcitx5-remote -n`, 均不可用降频至 5s), 经 `ime_short_label` 映射为托盘风格短标签
  (xkb:us::eng→En、拼音系→中、mozc→あ、hangul→한, 有单元测试), 变化时 emit
  `ime-changed`; 前端监听后**整树 render** 更新键帽(不要改成 textContent 局部更新,
  无合成器环境会有局部重绘残影), 初始值经 `get_ime_label` 命令获取; 标签为运行时值,
  不持久化。检测需会话 D-Bus 环境。
- **行内布局为步距模型**(main.js `rowHtml` + style.css `.row .key`): 每键占位 =
  权重/总权重(--total 由 JS 算出写入行内联样式) × 行宽, 键间隔含在步距内(margin 实现,
  末键吸收) —— **累计权重相同的键边界跨行像素级对齐**。完整模式第 4/5 行同栅格:
  空格右缘↔m 键右缘、←↔?/、↓↔↑、→ 与中 合计正好铺满右 Shift(2.15u); 改行权重时须保持
  该对齐关系。
- 窗口顶栏结构(main.js `render()`): 完整模式为共享 `.titlebar`(⚙ | 拖动把手 | ✕)+
  `.toolbar`(布局/窗口模式快捷键+弹簧把手)+单块 `.panel`; 分裂模式无共享顶栏, 两块
  `.panel.half` 各带内嵌 `.titlebar`(左: ⚙|布局|把手, 右: 把手|窗口|✕), 中缝穿透、
  两半互不相连; 顶栏/面板整体上报穿透区域(`sendRegions`), 标题栏空白处均可拖动。
  ⚙ 打开**独立设置窗口**(命令 `toggle_settings`), 不再有内嵌设置面板。
- **分裂宽度设置**(`split_half_units`, 键距数 5.0–9.5, 仅分裂布局): 纯前端几何 ——
  面板宽 = `--u × --su`(--su 由 render 写在 #app), 上限 46% 窗宽防两半重叠;
  键距 `--u` 与窗口高度仍按 SPLIT_HALF_UNITS=6.8 从屏幕宽推算, 与该设置无关。
- 窗口契约:四个窗口 label 为 `keyboard`、`ball`、`settings`、`touchpad`; 键盘/悬浮球的
  穿透区域以 CSS 像素上报, 后端负责 ×scale_factor 转物理像素; 设置与触摸板窗口全矩形
  可交互, 无需上报区域。托盘菜单项: 显示/隐藏键盘、触摸板(CheckMenuItem 勾选态随窗口
  显隐)、切换布局、重新适配、悬浮球、设置、退出。
