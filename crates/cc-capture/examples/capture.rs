//! `cargo run -p cc-capture --example capture` — lists cameras, grabs a few
//! frames from the first one and prints the effective frame rate.

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    if !cc_capture::request_permission(std::time::Duration::from_secs(60)) {
        anyhow::bail!("camera permission denied");
    }
    for d in cc_capture::list_devices()? {
        println!("{} [{}]", d.name, d.id);
    }
    let name = std::env::args().nth(1);
    let mut session = cc_capture::CaptureSession::open(name.as_deref(), Some(720))?;
    println!("opened {} — {}", session.device().name, session.format());
    let start = Instant::now();
    let mut n = 0;
    while start.elapsed().as_secs_f32() < 3.0 {
        let f = session.next_frame()?;
        n += 1;
        if n == 1 {
            println!("first frame {}x{}", f.width, f.height);
        }
    }
    println!("{:.1} fps", n as f32 / start.elapsed().as_secs_f32());
    Ok(())
}
