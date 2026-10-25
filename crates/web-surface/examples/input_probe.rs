//! Manual probe: verify keyboard and wheel injection against a local test page.
//!
//! Usage: cargo run -p vrcx-0-web-surface --example input_probe
//!
//! Writes `C:/T/probe-*.png` after each step.

use std::time::Duration;

const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><style>
body { margin:0; padding:16px; background:#101422; color:#e8ecff; font:15px "Microsoft YaHei UI",sans-serif; }
h1 { font-size:18px; margin:0 0 10px; }
input { width:94%; font-size:22px; padding:10px; border-radius:10px; border:1px solid #3a4570;
        background:#1b2340; color:#e8ecff; }
#list { margin-top:14px; height:420px; overflow:auto; border:1px solid #2a3456; border-radius:10px; }
.row { padding:9px 12px; border-bottom:1px solid #232c4a; font-size:15px; }
</style></head>
<body>
<h1>输入探针</h1>
<input id="q" placeholder="type here" autofocus>
<div id="list"></div>
<script>
  const list = document.getElementById('list');
  let html = '';
  for (let i = 1; i <= 60; i++) html += '<div class="row">第 ' + i + ' 行 · row ' + i + '</div>';
  list.innerHTML = html;
  const q = document.getElementById('q');
  q.addEventListener('input', () => { q.style.borderColor = '#4adfa0'; });
</script>
</body></html>
"#;

fn save(surface: &vrcx_0_web_surface::WebSurface, name: &str) {
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(250));
        if let Some(frame) = surface.latest_frame() {
            if frame.seq >= 2 {
                if let Some(image) =
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba.to_vec())
                {
                    let path = format!("C:/T/probe-{name}.png");
                    match image.save(&path) {
                        Ok(()) => {
                            println!("saved {path} (seq={})", frame.seq);
                            return;
                        }
                        Err(error) => eprintln!("save failed: {error}"),
                    }
                }
            }
        }
    }
    eprintln!("no frame for {name}");
}

fn main() {
    let dir = std::path::PathBuf::from("C:/T/probe-page");
    std::fs::create_dir_all(&dir).expect("create probe dir");
    std::fs::write(dir.join("index.html"), PAGE).expect("write probe page");

    let server = vrcx_0_web_surface::StaticServer::spawn(dir.clone()).expect("static server");
    let url = format!("{}index.html", server.base_url());
    println!("serving {url}");

    let mut cfg = vrcx_0_web_surface::WebSurfaceConfig::new(url);
    cfg.width = 640;
    cfg.height = 640;
    cfg.refresh_ms = 120;
    let surface = vrcx_0_web_surface::WebSurface::spawn(cfg).expect("web surface");

    std::thread::sleep(Duration::from_secs(6));
    save(&surface, "0-baseline");

    // 1) keyboard: focus the input by clicking it, then type "hello"
    surface.send(vrcx_0_web_surface::WebInputEvent::PointerDown { x: 320, y: 60 });
    std::thread::sleep(Duration::from_millis(80));
    surface.send(vrcx_0_web_surface::WebInputEvent::PointerUp { x: 320, y: 60 });
    std::thread::sleep(Duration::from_millis(400));
    for ch in "hello 星遇".chars() {
        surface.send(vrcx_0_web_surface::WebInputEvent::Key { vk: 0, down: true });
        surface.send(vrcx_0_web_surface::WebInputEvent::Char { ch });
        surface.send(vrcx_0_web_surface::WebInputEvent::Key { vk: 0, down: false });
        std::thread::sleep(Duration::from_millis(60));
    }
    std::thread::sleep(Duration::from_millis(600));
    save(&surface, "1-typed");

    // 2) wheel: scroll the list down
    for _ in 0..4 {
        surface.send(vrcx_0_web_surface::WebInputEvent::Wheel {
            x: 320,
            y: 400,
            delta: -120,
        });
        std::thread::sleep(Duration::from_millis(120));
    }
    std::thread::sleep(Duration::from_millis(600));
    save(&surface, "2-scrolled");

    // 3) hot reload: rewrite the page, ask the surface to reload
    std::fs::write(
        dir.join("index.html"),
        PAGE.replace("输入探针", "已更新 v2"),
    )
    .expect("rewrite page");
    surface.send(vrcx_0_web_surface::WebInputEvent::Reload);
    std::thread::sleep(Duration::from_millis(900));
    save(&surface, "3-reloaded");

    println!("done");
}
