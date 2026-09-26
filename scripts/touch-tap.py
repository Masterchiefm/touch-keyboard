#!/usr/bin/env python3
"""通过 uinput 创建虚拟触摸屏, 执行真实触摸 (Wayland 原生 wl_touch 路径).
用法:
  sg input -c "python3 touch-tap.py X Y [再敲的键码]"        # 轻触
  sg input -c "python3 touch-tap.py drag X1 Y1 X2 Y2 [ms]"   # 按下滑动到目标再抬起
  sg input -c "python3 touch-tap.py down X Y"                # 只按下 (配合后续命令)
  sg input -c "python3 touch-tap.py up"                      # 只抬起
  sg input -c "python3 touch-tap.py warp X Y"                # 画笔悬停把光标绝对移动到 X,Y
"""
import ctypes, os, struct, sys, time, fcntl

UI_SET_EVBIT  = 0x40045564
UI_SET_KEYBIT = 0x40045565
UI_SET_ABSBIT = 0x40045566
UI_DEV_CREATE = 0x5501
ABS_X, ABS_Y = 0x00, 0x01
BTN_TOUCH = 0x14A
EV_SYN, EV_KEY, EV_ABS = 0, 1, 3

class InputId(ctypes.Structure):
    _fields_ = [("bustype", ctypes.c_uint16), ("vendor", ctypes.c_uint16),
                ("product", ctypes.c_uint16), ("version", ctypes.c_uint16)]

class Setup(ctypes.Structure):
    _fields_ = [("name", ctypes.c_uint8 * 80), ("id", InputId),
                ("ff_effects_max", ctypes.c_uint32),
                ("absmax", ctypes.c_int32 * 64), ("absmin", ctypes.c_int32 * 64),
                ("absfuzz", ctypes.c_int32 * 64), ("absflat", ctypes.c_int32 * 64)]

W, H = 1440, 960  # 与桌面逻辑分辨率一致


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "tap"

    if mode == "warp":
        # 画笔悬停设备: 绝对移动光标 (无点击)。与触摸屏分开建设备。
        fd = os.open("/dev/uinput", os.O_RDWR)
        BTN_TOOL_PEN = 0x140
        for req, val in [(UI_SET_EVBIT, EV_KEY), (UI_SET_EVBIT, EV_ABS), (UI_SET_EVBIT, EV_SYN),
                         (UI_SET_KEYBIT, BTN_TOOL_PEN), (UI_SET_ABSBIT, ABS_X), (UI_SET_ABSBIT, ABS_Y)]:
            fcntl.ioctl(fd, req, val)
        s = Setup()
        s.name[:11] = b"TestWarpPen"
        s.id.bustype = 3
        s.absmax[ABS_X] = 65535
        s.absmax[ABS_Y] = 65535
        os.write(fd, bytes(s))
        fcntl.ioctl(fd, UI_DEV_CREATE)
        time.sleep(0.5)

        def emit(t, c, v):
            os.write(fd, struct.pack("qqhhi", 0, 0, t, c, v))

        x, y = int(sys.argv[2]), int(sys.argv[3])
        nx, ny = round(x * 65535 / W), round(y * 65535 / H)
        emit(EV_KEY, BTN_TOOL_PEN, 1)
        emit(EV_ABS, ABS_X, nx); emit(EV_ABS, ABS_Y, ny); emit(EV_SYN, 0, 0)
        time.sleep(0.06)
        emit(EV_KEY, BTN_TOOL_PEN, 0); emit(EV_SYN, 0, 0)
        time.sleep(0.2)
        os.close(fd)
        print(f"warped to {x},{y}")
        return

    fd = os.open("/dev/uinput", os.O_RDWR)
    for req, val in [(UI_SET_EVBIT, EV_KEY), (UI_SET_EVBIT, EV_ABS), (UI_SET_EVBIT, EV_SYN),
                     (UI_SET_KEYBIT, BTN_TOUCH), (UI_SET_KEYBIT, 45), (UI_SET_ABSBIT, ABS_X), (UI_SET_ABSBIT, ABS_Y)]:
        fcntl.ioctl(fd, req, val)
    s = Setup()
    s.name[:9] = b"TestTouch"
    s.id.bustype = 3
    s.absmax[ABS_X] = W
    s.absmax[ABS_Y] = H
    os.write(fd, bytes(s))
    fcntl.ioctl(fd, UI_DEV_CREATE)
    time.sleep(0.5)  # 等 mutter 识别设备

    def emit(t, c, v):
        os.write(fd, struct.pack("qqhhi", 0, 0, t, c, v))

    def move_to(tx, ty):
        emit(EV_ABS, ABS_X, tx); emit(EV_ABS, ABS_Y, ty); emit(EV_SYN, 0, 0)

    def touch_down(tx, ty):
        move_to(tx, ty)
        emit(EV_KEY, BTN_TOUCH, 1); emit(EV_SYN, 0, 0)

    def touch_up():
        emit(EV_KEY, BTN_TOUCH, 0); emit(EV_SYN, 0, 0)

    if mode == "tap":
        x, y = int(sys.argv[2]), int(sys.argv[3])
        key_after = int(sys.argv[4]) if len(sys.argv) > 4 else None
        touch_down(x, y)
        time.sleep(0.09)
        touch_up()
        time.sleep(0.15)
        if key_after is not None:
            time.sleep(0.3)
            emit(EV_KEY, key_after, 1); emit(EV_SYN, 0, 0)
            time.sleep(0.05)
            emit(EV_KEY, key_after, 0); emit(EV_SYN, 0, 0)
        print(f"tapped {x},{y}" + (f" + key {key_after}" if key_after else ""))
    elif mode == "drag":
        x1, y1 = int(sys.argv[2]), int(sys.argv[3])
        x2, y2 = int(sys.argv[4]), int(sys.argv[5])
        ms = int(sys.argv[6]) if len(sys.argv) > 6 else 400
        steps = max(2, ms // 20)
        touch_down(x1, y1)
        time.sleep(0.06)
        for i in range(1, steps + 1):
            move_to(round(x1 + (x2 - x1) * i / steps), round(y1 + (y2 - y1) * i / steps))
            time.sleep(ms / 1000.0 / steps)
        time.sleep(0.05)
        touch_up()
        print(f"dragged ({x1},{y1}) -> ({x2},{y2}) in {ms}ms")
    elif mode == "down":
        touch_down(int(sys.argv[2]), int(sys.argv[3]))
        time.sleep(3.0)  # 停留一会儿供观察 (之后设备关闭即触摸结束)
        print(f"held {sys.argv[2]},{sys.argv[3]}")
    elif mode == "up":
        touch_up()
        print("up")
    else:
        print(f"未知模式: {mode}")
        sys.exit(2)
    time.sleep(0.3)
    os.close(fd)


if __name__ == "__main__":
    main()
