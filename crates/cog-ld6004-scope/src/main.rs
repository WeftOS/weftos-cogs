//! Bench console for an HLK-LD6004 on a USB-UART, the companion app for the
//! `ld6004-radar` cog (src/cogs/ld6004-radar).
//! `weft-ld6004-scope [--probe] [--port /dev/cu.usbserial-214110]`

mod app;
mod link;
mod proto;

fn main() {
    let (port, probe) = args();
    if probe {
        match link::probe(&port) {
            Ok(text) => {
                println!("{text}");
            }
            Err(text) => {
                eprintln!("{text}");
                std::process::exit(1);
            }
        }
        return;
    }
    let app = app::ScopeApp::open(&port);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_title("WeftOS LD6004"),
        ..Default::default()
    };
    if let Err(err) = eframe::run_native(
        "WeftOS LD6004",
        options,
        Box::new(move |_cc| Ok(Box::new(app))),
    ) {
        eprintln!("window: {err}");
        std::process::exit(1);
    }
}

fn args() -> (String, bool) {
    let mut port = link::DEFAULT_PORT.to_string();
    let mut probe = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--probe" => probe = true,
            "--port" => {
                if let Some(next) = args.next() {
                    port = next;
                }
            }
            "--help" | "-h" => {
                eprintln!("weft-ld6004-scope [--probe] [--port PATH]");
                std::process::exit(0);
            }
            other if !other.starts_with('-') => port = other.to_string(),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    (port, probe)
}
