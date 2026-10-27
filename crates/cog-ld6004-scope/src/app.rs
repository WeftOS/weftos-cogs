//! Live view for one HLK-LD6004: XY track, Z, presence lamps, protocol
//! checklist, and Apply buttons. Nothing is written until a button is clicked.

use crate::link::{self, Cmd, Handle};
use crate::proto::{
    self, Body, Snapshot, TY_DELAY_SET, TY_DETECTION, TY_DWELL, TY_INTERFERENCE, TY_INTERVAL_SET,
    TY_LIFE_SET, TY_SLEEP_SET, TY_Z_SET, TY_ZONE_SET, Track, Verdict, Zone,
};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{HLine, Plot, PlotPoints, Points, VLine};
use std::time::Duration;

const GREEN: Color32 = Color32::from_rgb(90, 210, 130);
const AMBER: Color32 = Color32::from_rgb(230, 170, 60);
const RED: Color32 = Color32::from_rgb(220, 90, 90);

#[derive(Default)]
struct Touch {
    sens: bool,
    trig: bool,
    install: bool,
    mode: bool,
    p20: bool,
    delay: bool,
    sleep: bool,
    life: bool,
    interval: bool,
    z: bool,
    zone: bool,
}

#[derive(Default)]
struct Have {
    sens: bool,
    trig: bool,
    install: bool,
    mode: bool,
    p20: bool,
    delay: bool,
    sleep: bool,
    life: bool,
    interval: bool,
    z: bool,
}

pub struct ScopeApp {
    tx: std::sync::mpsc::Sender<Cmd>,
    shared: std::sync::Arc<std::sync::Mutex<link::Shared>>,
    port_edit: String,
    touch: Touch,
    have: Have,
    sens: u8,
    trig: u8,
    install: u8,
    mode: u32,
    p20: u8,
    delay: u32,
    sleep: u32,
    life: u32,
    interval: u32,
    z_min: f32,
    z_max: f32,
    zone_kind: usize,
    zone_slot: usize,
    zx0: f32,
    zx1: f32,
    zy0: f32,
    zy1: f32,
    zz0: f32,
    zz1: f32,
    _handle: Handle,
}

struct Hud {
    status: String,
    open: bool,
    frames: u64,
    bytes: u64,
    resyncs: u64,
    rate: f32,
}

impl ScopeApp {
    pub fn open(port: &str) -> Self {
        let handle = link::start(port).expect("ld6004 thread");
        Self {
            tx: handle.tx.clone(),
            shared: std::sync::Arc::clone(&handle.shared),
            port_edit: port.to_string(),
            touch: Touch::default(),
            have: Have::default(),
            sens: 2,
            trig: 2,
            install: 0,
            mode: 0,
            p20: 0,
            delay: 30,
            sleep: 100,
            life: 200,
            interval: 1,
            z_min: 0.5,
            z_max: 6.0,
            zone_kind: 1,
            zone_slot: 0,
            zx0: -6.0,
            zx1: 6.0,
            zy0: -6.0,
            zy1: 6.0,
            zz0: 0.5,
            zz1: 6.0,
            _handle: handle,
        }
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    fn apply_control(&self, cmd: u32) {
        self.send(Cmd::Control(cmd));
        self.send(Cmd::QueryAll);
    }

    fn apply_frame(&self, ty: u16, data: Vec<u8>) {
        self.send(Cmd::Frame { ty, data });
        self.send(Cmd::QueryAll);
    }

    fn sync(&mut self, snap: &Snapshot) {
        pull_u8(
            &mut self.sens,
            &mut self.touch.sens,
            &mut self.have.sens,
            snap.sensitivity,
        );
        pull_u8(
            &mut self.trig,
            &mut self.touch.trig,
            &mut self.have.trig,
            snap.trigger,
        );
        pull_u8(
            &mut self.install,
            &mut self.touch.install,
            &mut self.have.install,
            snap.install,
        );
        pull_u8(
            &mut self.p20,
            &mut self.touch.p20,
            &mut self.have.p20,
            snap.p20,
        );
        if let Some(v) = snap.mode
            && matches!(v, 0 | 1 | 4)
        {
            if self.touch.mode && self.mode == v {
                self.touch.mode = false;
            }
            if !self.touch.mode {
                self.mode = v;
                self.have.mode = true;
            }
        }
        pull_u32(
            &mut self.delay,
            &mut self.touch.delay,
            &mut self.have.delay,
            snap.delay_s,
        );
        pull_u32(
            &mut self.sleep,
            &mut self.touch.sleep,
            &mut self.have.sleep,
            snap.sleep_ms,
        );
        pull_u32(
            &mut self.life,
            &mut self.touch.life,
            &mut self.have.life,
            snap.life,
        );
        pull_u32(
            &mut self.interval,
            &mut self.touch.interval,
            &mut self.have.interval,
            snap.interval,
        );
        if self.interval == 0 {
            self.interval = 1;
        }
        if let (Some(a), Some(b)) = (snap.z_min, snap.z_max) {
            if self.touch.z && near(self.z_min, a) && near(self.z_max, b) {
                self.touch.z = false;
            }
            if !self.touch.z {
                self.z_min = a;
                self.z_max = b;
                self.have.z = true;
            }
        }
        if self.touch.zone {
            if self.zone_matches(snap) {
                self.touch.zone = false;
            }
        } else {
            self.load_zone(snap);
        }
    }

    fn load_zone(&mut self, snap: &Snapshot) {
        if !self.group_ready(snap) {
            return;
        }
        let z = group(snap, self.zone_kind)[self.zone_slot];
        self.zx0 = z.x_min;
        self.zx1 = z.x_max;
        self.zy0 = z.y_min;
        self.zy1 = z.y_max;
        self.zz0 = z.z_min;
        self.zz1 = z.z_max;
    }

    fn group_ready(&self, snap: &Snapshot) -> bool {
        match self.zone_kind {
            0 => snap.got(TY_INTERFERENCE),
            1 => snap.got(TY_DETECTION),
            _ => snap.got(TY_DWELL),
        }
    }

    fn zone_matches(&self, snap: &Snapshot) -> bool {
        if !self.group_ready(snap) {
            return false;
        }
        let z = group(snap, self.zone_kind)[self.zone_slot];
        near(self.zx0, z.x_min)
            && near(self.zx1, z.x_max)
            && near(self.zy0, z.y_min)
            && near(self.zy1, z.y_max)
            && near(self.zz0, z.z_min)
            && near(self.zz1, z.z_max)
    }

    fn editor_zone(&self) -> Zone {
        Zone {
            x_min: self.zx0,
            x_max: self.zx1,
            y_min: self.zy0,
            y_max: self.zy1,
            z_min: self.zz0,
            z_max: self.zz1,
        }
    }
}

impl eframe::App for ScopeApp {
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(33));
        let (snap, hud) = {
            let Ok(guard) = self.shared.lock() else {
                return;
            };
            (
                guard.snap.clone(),
                Hud {
                    status: guard.status.clone(),
                    open: guard.open,
                    frames: guard.frames,
                    bytes: guard.bytes,
                    resyncs: guard.resyncs,
                    rate: guard.pairs_per_s,
                },
            )
        };
        self.sync(&snap);
        egui::Panel::top("bar").show_inside(ui, |ui| self.top(ui, &snap, &hud));
        egui::Panel::bottom("tune")
            .exact_size(248.0)
            .show_inside(ui, |ui| self.tune(ui, &snap));
        let bodies = proto::steady_bodies(&snap.trail, &snap.tracks, snap.target_miss);
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.horizontal_top(|ui| {
                let plot_w = ui.available_width() * 0.64;
                let h = ui.available_height();
                ui.vertical(|ui| {
                    ui.set_width(plot_w);
                    self.xz(ui, &snap, &bodies, (h * 0.62).max(200.0));
                    self.xy(ui, &snap, &bodies, (h * 0.36).max(140.0));
                });
                ui.vertical(|ui| {
                    self.side(ui, &snap, &bodies);
                });
            });
        });
    }
}

impl ScopeApp {
    fn top(&mut self, ui: &mut egui::Ui, snap: &Snapshot, hud: &Hud) {
        ui.horizontal(|ui| {
            ui.heading("LD6004");
            ui.add(egui::TextEdit::singleline(&mut self.port_edit).desired_width(280.0));
            if ui.button("Open").clicked() {
                self.send(Cmd::Reopen(self.port_edit.clone()));
            }
            if ui.button("Query all").clicked() {
                self.send(Cmd::QueryAll);
            }
            let color = if hud.open { GREEN } else { RED };
            ui.label(RichText::new(&hud.status).color(color));
            ui.monospace(format!(
                "{} frames   {} B   {:.0}/s",
                hud.frames, hud.bytes, hud.rate
            ));
            if hud.resyncs > 0 {
                ui.label(RichText::new(format!("{} resyncs", hud.resyncs)).color(AMBER));
            }
            if let Some(fw) = snap.firmware {
                ui.label(proto::firmware_text(fw));
            }
        });
    }

    fn side(&self, ui: &mut egui::Ui, snap: &Snapshot, bodies: &[Body]) {
        let title = match bodies.len() {
            0 => "Clear".to_string(),
            1 => "1 person".to_string(),
            n => format!("{n} people"),
        };
        let present = snap.presence.iter().any(|z| *z != 0);
        ui.label(
            RichText::new(if bodies.is_empty() && present {
                "Occupied"
            } else {
                &title
            })
            .size(22.0),
        );
        if bodies.is_empty() {
            let text = if present {
                "Zone still occupied. The coordinate dropped."
            } else {
                "No one in the detection zone."
            };
            ui.label(RichText::new(text).color(if present { AMBER } else { Color32::GRAY }));
        }
        for body in bodies {
            person_block(ui, body);
        }
        ui.label(
            RichText::new(presence_line(&snap.presence))
                .small()
                .color(Color32::LIGHT_GRAY),
        );
        if !snap.cloud.is_empty() && !bodies.is_empty() {
            let near = snap
                .cloud
                .iter()
                .filter(|t| near_person(t, bodies, 0.7))
                .count();
            ui.label(
                RichText::new(format!(
                    "{near} of {} raw points sit on the person. The rest are hidden.",
                    snap.cloud.len()
                ))
                .small()
                .color(Color32::LIGHT_GRAY),
            );
        }
        ui.separator();
        egui::CollapsingHeader::new("Protocol checklist")
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for row in proto::rows(snap) {
                            let (mark, color) = match row.verdict {
                                Verdict::Pass => ("PASS", GREEN),
                                Verdict::Wait => ("WAIT", Color32::GRAY),
                                Verdict::Held => ("HELD", AMBER),
                            };
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(mark).color(color).monospace());
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&row.name).monospace());
                                    ui.label(
                                        RichText::new(&row.detail)
                                            .small()
                                            .color(Color32::LIGHT_GRAY),
                                    );
                                });
                            });
                        }
                    });
            });
    }

    fn xz(&self, ui: &mut egui::Ui, snap: &Snapshot, bodies: &[Body], height: f32) {
        Plot::new("xz")
            .height(height)
            .data_aspect(1.0)
            .default_x_bounds(-2.0, 2.0)
            .default_y_bounds(0.0, 2.5)
            .x_axis_label("x  lateral  m")
            .y_axis_label("z  out from antenna  m")
            .show(ui, |p| {
                p.hline(HLine::new("1 m", 1.0).color(AMBER));
                if let Some(z) = snap.z_min {
                    p.hline(HLine::new("z min", z).color(Color32::from_white_alpha(50)));
                }
                p.points(
                    Points::new("sensor", PlotPoints::new(vec![[0.0, 0.0]]))
                        .radius(4.0)
                        .color(Color32::WHITE),
                );
                plot_people(p, snap, bodies, true);
            });
    }

    fn xy(&self, ui: &mut egui::Ui, snap: &Snapshot, bodies: &[Body], height: f32) {
        let camera = proto::CAMERA_ABOVE_RADAR_M as f64;
        Plot::new("front")
            .height(height)
            .data_aspect(1.0)
            .default_x_bounds(-1.2, 1.2)
            .default_y_bounds(-0.4, 1.0)
            .x_axis_label("x  lateral  m")
            .y_axis_label("up from radar  m")
            .show(ui, |p| {
                p.hline(HLine::new("camera", camera).color(Color32::from_rgb(120, 170, 230)));
                p.hline(HLine::new("radar", 0.0).color(Color32::from_white_alpha(40)));
                p.vline(VLine::new("center", 0.0).color(Color32::from_white_alpha(30)));
                plot_people(p, snap, bodies, false);
            });
    }

    fn tune(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.label(
            RichText::new(
                "Apply writes the module. Baud, OTA, radar-off, and the clear/reset commands are not here.",
            )
            .small()
            .color(AMBER),
        );
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.tune_rows(ui, snap);
        });
    }

    fn tune_rows(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.horizontal(|ui| {
            ui.label("sensitivity");
            choices(
                ui,
                &mut self.sens,
                &mut self.touch.sens,
                &[(0, "low"), (1, "medium"), (2, "high")],
            );
            if apply_btn(ui, self.have.sens) {
                self.touch.sens = true;
                self.apply_control(proto::sens_command(self.sens));
            }
            module_u8(ui, snap.sensitivity, proto::sens_name);
        });
        ui.horizontal(|ui| {
            ui.label("trigger");
            choices(
                ui,
                &mut self.trig,
                &mut self.touch.trig,
                &[(0, "slow"), (1, "medium"), (2, "fast")],
            );
            if apply_btn(ui, self.have.trig) {
                self.touch.trig = true;
                self.apply_control(proto::trigger_command(self.trig));
            }
            module_u8(ui, snap.trigger, proto::trigger_name);
        });
        ui.horizontal(|ui| {
            ui.label("install");
            choices(
                ui,
                &mut self.install,
                &mut self.touch.install,
                &[(0, "top"), (1, "side")],
            );
            if apply_btn(ui, self.have.install) {
                self.touch.install = true;
                self.apply_control(proto::install_command(self.install));
            }
            module_u8(ui, snap.install, proto::install_name);
        });
        ui.horizontal(|ui| {
            ui.label("mode");
            choices(
                ui,
                &mut self.mode,
                &mut self.touch.mode,
                &[(0, "normal"), (1, "unattended"), (4, "strong reflection")],
            );
            if apply_btn(ui, self.have.mode)
                && let Some(cmd) = proto::mode_command(self.mode)
            {
                self.touch.mode = true;
                self.apply_control(cmd);
            }
            ui.label(snap.mode.map(proto::mode_name).unwrap_or("module —"));
        });
        ui.horizontal(|ui| {
            ui.label("OUT / P20");
            egui::ComboBox::from_id_salt("p20")
                .selected_text(proto::p20_name(self.p20))
                .show_ui(ui, |ui| {
                    for v in 0..6 {
                        if ui
                            .selectable_value(&mut self.p20, v, proto::p20_name(v))
                            .changed()
                        {
                            self.touch.p20 = true;
                        }
                    }
                });
            if apply_btn(ui, self.have.p20)
                && let Some(cmd) = proto::p20_command(self.p20)
            {
                self.touch.p20 = true;
                self.apply_control(cmd);
            }
            module_u8(ui, snap.p20, proto::p20_name);
        });
        if num_row(
            ui,
            "hold",
            "s",
            &mut self.delay,
            0..=600,
            &mut self.touch.delay,
            self.have.delay,
            snap.delay_s,
        ) {
            let bytes = self.delay.to_le_bytes().to_vec();
            self.apply_frame(TY_DELAY_SET, bytes);
        }
        if num_row(
            ui,
            "sleep",
            "ms",
            &mut self.sleep,
            0..=5_000,
            &mut self.touch.sleep,
            self.have.sleep,
            snap.sleep_ms,
        ) {
            let bytes = self.sleep.to_le_bytes().to_vec();
            self.apply_frame(TY_SLEEP_SET, bytes);
        }
        if num_row(
            ui,
            "dwell life",
            "counts",
            &mut self.life,
            0..=3_000,
            &mut self.touch.life,
            self.have.life,
            snap.life,
        ) {
            let bytes = self.life.to_le_bytes().to_vec();
            self.apply_frame(TY_LIFE_SET, bytes);
        }
        if num_row(
            ui,
            "interval",
            "frames",
            &mut self.interval,
            1..=60,
            &mut self.touch.interval,
            self.have.interval,
            snap.interval,
        ) {
            let bytes = self.interval.to_le_bytes().to_vec();
            self.apply_frame(TY_INTERVAL_SET, bytes);
        }
        ui.horizontal(|ui| {
            ui.label("Z range");
            if ui
                .add(
                    egui::Slider::new(&mut self.z_min, 0.0..=8.0)
                        .step_by(0.1)
                        .text("min"),
                )
                .changed()
            {
                self.touch.z = true;
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.z_max, 0.0..=8.0)
                        .step_by(0.1)
                        .text("max"),
                )
                .changed()
            {
                self.touch.z = true;
            }
            if apply_btn(ui, self.have.z) {
                self.touch.z = true;
                self.apply_frame(TY_Z_SET, proto::z_payload(self.z_min, self.z_max));
            }
            match (snap.z_min, snap.z_max) {
                (Some(a), Some(b)) => ui.label(format!("module {a:.2} .. {b:.2} m")),
                _ => ui.label("module —"),
            };
        });
        ui.horizontal(|ui| {
            ui.label("zone");
            let before = (self.zone_kind, self.zone_slot);
            choices(
                ui,
                &mut self.zone_kind,
                &mut self.touch.zone,
                &[(0, "interference"), (1, "detection"), (2, "dwell")],
            );
            choices(
                ui,
                &mut self.zone_slot,
                &mut self.touch.zone,
                &[(0, "0"), (1, "1"), (2, "2"), (3, "3")],
            );
            if (self.zone_kind, self.zone_slot) != before {
                self.touch.zone = false;
                self.load_zone(snap);
            }
            ui.label(format!("id {}", self.zone_kind * 4 + self.zone_slot));
        });
        ui.horizontal(|ui| {
            zone_slider(ui, &mut self.zx0, "x min", &mut self.touch.zone);
            zone_slider(ui, &mut self.zx1, "x max", &mut self.touch.zone);
            zone_slider(ui, &mut self.zy0, "y min", &mut self.touch.zone);
            zone_slider(ui, &mut self.zy1, "y max", &mut self.touch.zone);
        });
        ui.horizontal(|ui| {
            zone_slider(ui, &mut self.zz0, "z min", &mut self.touch.zone);
            zone_slider(ui, &mut self.zz1, "z max", &mut self.touch.zone);
            if apply_btn(ui, self.group_ready(snap)) {
                let area = (self.zone_kind * 4 + self.zone_slot) as i32;
                self.touch.zone = true;
                self.apply_frame(TY_ZONE_SET, proto::zone_payload(area, self.editor_zone()));
            }
            ui.label("one zone per Apply");
        });
        ui.horizontal(|ui| {
            ui.label("stream");
            if ui.button("Show point cloud").clicked() {
                self.send(Cmd::Control(0x06));
            }
            if ui.button("Hide point cloud").clicked() {
                self.send(Cmd::Control(0x07));
            }
            if ui.button("Targets on").clicked() {
                self.send(Cmd::Control(0x08));
            }
            if ui.button("Targets off").clicked() {
                self.send(Cmd::Control(0x09));
            }
            ui.label(RichText::new("click to send").small().color(Color32::GRAY));
        });
    }
}

#[allow(clippy::too_many_arguments)] // one slider row: label, unit, value, range, and its apply state
fn num_row(
    ui: &mut egui::Ui,
    label: &str,
    unit: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    touch: &mut bool,
    have: bool,
    live: Option<u32>,
) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.add(egui::Slider::new(value, range).text(unit)).changed() {
            *touch = true;
        }
        if apply_btn(ui, have) {
            *touch = true;
            clicked = true;
        }
        match live {
            Some(v) => ui.label(format!("module {v}")),
            None => ui.label("module —"),
        };
    });
    clicked
}

fn pull_u8(slot: &mut u8, touch: &mut bool, have: &mut bool, live: Option<u8>) {
    let Some(v) = live else { return };
    if *touch && *slot == v {
        *touch = false;
    }
    if !*touch {
        *slot = v;
        *have = true;
    }
}

fn pull_u32(slot: &mut u32, touch: &mut bool, have: &mut bool, live: Option<u32>) {
    let Some(v) = live else { return };
    if *touch && *slot == v {
        *touch = false;
    }
    if !*touch {
        *slot = v;
        *have = true;
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.02
}

fn group(snap: &Snapshot, kind: usize) -> &[Zone; 4] {
    match kind {
        0 => &snap.interference,
        1 => &snap.detection,
        _ => &snap.dwell,
    }
}

fn choices<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    value: &mut T,
    touch: &mut bool,
    items: &[(T, &str)],
) {
    for (v, name) in items {
        if ui.selectable_value(value, *v, *name).changed() {
            *touch = true;
        }
    }
}

fn apply_btn(ui: &mut egui::Ui, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new("Apply"))
        .clicked()
}

fn module_u8(ui: &mut egui::Ui, live: Option<u8>, name: fn(u8) -> &'static str) {
    ui.label(live.map(name).unwrap_or("module —"));
}

fn zone_slider(ui: &mut egui::Ui, value: &mut f32, text: &str, touch: &mut bool) {
    if ui
        .add(egui::Slider::new(value, -8.0..=8.0).step_by(0.1).text(text))
        .changed()
    {
        *touch = true;
    }
}

fn person_block(ui: &mut egui::Ui, body: &Body) {
    ui.label(
        RichText::new(format!("{:.2} m", body.range))
            .size(32.0)
            .color(GREEN),
    );
    let up = proto::above_radar_m(body.y);
    let vs_camera = up - proto::CAMERA_ABOVE_RADAR_M;
    ui.monospace(format!(
        "x {:>+5.2} lateral    {:>+5.2} above radar    z {:>4.2} forward",
        body.x, up, body.z
    ));
    let vs = if vs_camera.abs() < 0.08 {
        "about level with the camera".to_string()
    } else if vs_camera > 0.0 {
        format!("{:.2} m above the camera", vs_camera)
    } else {
        format!("{:.2} m below the camera", -vs_camera)
    };
    ui.label(RichText::new(vs).small().color(Color32::LIGHT_GRAY));
    let motion = if body.doppler == 0 {
        "standing still".to_string()
    } else {
        format!("motion index {}", body.doppler)
    };
    ui.label(motion);
    if body.ids.len() > 1 {
        let ids = body
            .ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        ui.label(
            RichText::new(format!(
                "tracker ids {ids} are {:.2} m apart, shown as one person",
                body.spread
            ))
            .small()
            .color(Color32::LIGHT_GRAY),
        );
    }
}

fn presence_line(presence: &[u32; 4]) -> String {
    let on: Vec<String> = presence
        .iter()
        .enumerate()
        .filter(|(_, v)| **v != 0)
        .map(|(i, _)| i.to_string())
        .collect();
    if on.is_empty() {
        "zones clear".to_string()
    } else {
        format!("zone {} occupied", on.join(", "))
    }
}

fn near_person(track: &Track, bodies: &[Body], radius: f32) -> bool {
    let r2 = radius * radius;
    bodies.iter().any(|body| {
        let dx = track.x - body.x;
        let dy = track.y - body.y;
        let dz = track.z - body.z;
        dx * dx + dy * dy + dz * dz <= r2
    })
}

fn plot_people(p: &mut egui_plot::PlotUi<'_>, snap: &Snapshot, bodies: &[Body], xz: bool) {
    let cloud: Vec<[f64; 2]> = snap
        .cloud
        .iter()
        .filter(|t| near_person(t, bodies, 0.7))
        .map(|t| {
            if xz {
                [t.x as f64, t.z as f64]
            } else {
                [t.x as f64, proto::above_radar_m(t.y) as f64]
            }
        })
        .collect();
    if !cloud.is_empty() {
        p.points(
            Points::new("raw", PlotPoints::new(cloud))
                .radius(2.0)
                .color(Color32::from_white_alpha(80)),
        );
    }
    let now: Vec<[f64; 2]> = bodies
        .iter()
        .map(|body| {
            if xz {
                [body.x as f64, body.z as f64]
            } else {
                [body.x as f64, proto::above_radar_m(body.y) as f64]
            }
        })
        .collect();
    if !now.is_empty() {
        p.points(
            Points::new("person", PlotPoints::new(now))
                .radius(9.0)
                .color(GREEN),
        );
    }
}
