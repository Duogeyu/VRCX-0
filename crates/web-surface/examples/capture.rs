//! Manual smoke test: render a page offscreen, capture frames, save a PNG.
//!
//! Usage: cargo run -p vrcx-0-web-surface --example capture [url]

use std::time::Duration;

fn main() {
    let first = std::env::args().nth(1).unwrap_or_default();
    // `--plugin <dir>` serves a plugin directory with the built-in static server.
    let mut server_guard = None;
    let url = if first == "--plugin" {
        let dir = std::env::args().nth(2).expect("--plugin needs a directory");
        let server = vrcx_0_web_surface::StaticServer::spawn(std::path::PathBuf::from(dir))
            .expect("static server");
        let url = format!("{}index.html", server.base_url());
        server_guard = Some(server);
        url
    } else if first.is_empty() {
        "https://vrcc.cc/".to_string()
    } else {
        first
    };
    let _ = &server_guard;
    let mut cfg = vrcx_0_web_surface::WebSurfaceConfig::new(url.clone());
    cfg.width = 640;
    cfg.height = 640;
    cfg.refresh_ms = 200;
    println!("spawning web surface for {url}");

    let surface = match vrcx_0_web_surface::WebSurface::spawn(cfg) {
        Ok(surface) => surface,
        Err(error) => {
            eprintln!("spawn failed: {error}");
            std::process::exit(1);
        }
    };

    // Optional: --click x,y to verify pointer injection end to end.
    let click_arg = std::env::args()
        .nth(2)
        .filter(|value| !value.contains('\\') && value.contains(','));
    let click: Option<(i32, i32)> = click_arg.and_then(|arg| {
        let (x, y) = arg.split_once(',')?;
        Some((x.parse().ok()?, y.parse().ok()?))
    });
    let mut clicked = false;

    let mut saved = None;
    for step in 0..40 {
        std::thread::sleep(Duration::from_millis(500));
        let status = surface.status();
        if let Some((x, y)) = click {
            if !clicked && (step + 1) * 500 >= 4000 {
                println!("clicking at {x},{y}");
                surface.send(vrcx_0_web_surface::WebInputEvent::PointerDown { x, y });
                std::thread::sleep(Duration::from_millis(80));
                surface.send(vrcx_0_web_surface::WebInputEvent::PointerUp { x, y });
                clicked = true;
            }
        }
        match surface.latest_frame() {
            Some(frame) => {
                println!(
                    "[{}ms] frame {}x{} seq={} status={:?}",
                    (step + 1) * 500,
                    frame.width,
                    frame.height,
                    frame.seq,
                    status
                );
                if (step + 1) * 500 >= 7000 && saved.is_none() {
                    match image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba.to_vec())
                    {
                        Some(image) => {
                            let path = "C:/T/web-surface-capture.png";
                            if let Err(error) = image.save(path) {
                                eprintln!("save failed: {error}");
                            } else {
                                println!("saved {path}");
                                saved = Some(path.to_string());
                            }
                        }
                        None => eprintln!("frame buffer did not match its dimensions"),
                    }
                }
                if saved.is_some() {
                    break;
                }
            }
            None => println!(
                "[{}ms] waiting for first frame (status={:?})",
                (step + 1) * 500,
                status
            ),
        }
    }

    match saved {
        Some(path) => println!("OK: captured to {path}"),
        None => {
            eprintln!("FAILED: no frame was captured");
            std::process::exit(2);
        }
    }
}
