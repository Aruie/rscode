//! Development-only: render UI scenes into a PNG with the software renderer (no display needed).
//! `cargo run --features screenshot -- --screenshot <scene> <out.png>`

use crate::state::{with_state, State, STATE};
use crate::App;
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::ComponentHandle;
use std::rc::Rc;

struct Shot(Rc<MinimalSoftwareWindow>);

impl Platform for Shot {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

pub fn run(scene: &str, out: &str) {
    let (w, h) = (1280u32, 820u32);
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Shot(window.clone()))).unwrap();
    window.set_size(slint::PhysicalSize::new(w, h));
    let app = App::new().unwrap();
    STATE.with(|s| *s.borrow_mut() = Some(State::new(&app)));
    crate::wire(&app);
    app.show().unwrap();
    with_state(|s| s.shot_scene(scene));
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(60));
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        let mut buf = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
        window.draw_if_needed(|r| {
            r.render(&mut buf, w as usize);
        });
        if buf.iter().any(|p| p.alpha != 0) {
            let mut bytes = Vec::with_capacity(buf.len() * 3);
            for p in &buf {
                bytes.extend_from_slice(&[p.red, p.green, p.blue]);
            }
            let f = std::fs::File::create(out).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&bytes).unwrap();
        }
    }
    println!("wrote {out}");
}
