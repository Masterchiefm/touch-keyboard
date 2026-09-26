// 调试工具: 给指定窗口设置测试用输入区域 (左半可交互, 右半穿透)
//   cargo run --example set_shape -- 0xa0000d
use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{ClipOrdering, ConnectionExt as _, Rectangle};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = args.get(1).expect("用法: set_shape 0x窗口id [query|set]").clone();
    let win = if let Some(h) = arg.strip_prefix("0x") {
        u32::from_str_radix(h, 16).unwrap()
    } else {
        arg.parse().unwrap()
    };
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let v = conn.shape_query_version().unwrap().reply().unwrap();
    println!("shape ext: major={} minor={}", v.major_version, v.minor_version);

    if args.get(2).map(|s| s == "query").unwrap_or(false) {
        let r = conn.shape_get_rectangles(win, SK::INPUT).unwrap().reply().unwrap();
        let rects: Vec<(i16, i16, u16, u16)> = r
            .rectangles
            .iter()
            .map(|rc| (rc.x, rc.y, rc.width, rc.height))
            .collect();
        println!("input shape rects={:?}", rects);
        return;
    }

    conn.shape_rectangles(
        SO::SET,
        SK::INPUT,
        ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &[Rectangle { x: 0, y: 0, width: 600, height: 300 }],
    )
    .unwrap();
    conn.flush().unwrap();
    println!("已设置输入区域 (0,0 600x300)");
}
