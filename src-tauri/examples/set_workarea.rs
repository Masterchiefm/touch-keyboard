// 调试工具: 设置/读取 root 窗口的 _NET_WORKAREA
//   cargo run --example set_workarea -- set 0,0,1920,1000
//   cargo run --example set_workarea -- read
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, PropMode};
use x11rb::wrapper::ConnectionExt as WrapperExt;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let wa = conn.intern_atom(false, b"_NET_WORKAREA").unwrap().reply().unwrap().atom;
    let card = conn.intern_atom(false, b"CARDINAL").unwrap().reply().unwrap().atom;

    if args.get(1).map(|s| s == "read").unwrap_or(false) {
        let r = conn
            .get_property(false, root, wa, card, 0, 16)
            .unwrap()
            .reply()
            .unwrap();
        let v: Vec<u32> = r.value32().map(Iterator::collect).unwrap_or_default();
        println!("read: {:?}", v);
    } else {
        let v: Vec<i32> = args[2]
            .split(',')
            .map(|s| s.trim().parse().unwrap())
            .collect();
        assert_eq!(v.len(), 4, "需要 x,y,w,h 四个值");
        conn.change_property32(
            PropMode::REPLACE,
            root,
            wa,
            card,
            &[v[0] as u32, v[1] as u32, v[2] as u32, v[3] as u32],
        )
        .unwrap();
        conn.flush().unwrap();
        println!("set: {:?}", v);
        // 立即回读验证
        let r = conn
            .get_property(false, root, wa, card, 0, 16)
            .unwrap()
            .reply()
            .unwrap();
        let back: Vec<u32> = r.value32().map(Iterator::collect).unwrap_or_default();
        println!("回读: {:?}", back);
    }
}
