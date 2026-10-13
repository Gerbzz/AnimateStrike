//! AnimateStrike: a focused 2D character editor over the pinned EffectCraft model.
//! The default scene and all artwork made at startup are procedural and local.

#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod control;

use std::io::Write;
use std::path::Path;
use std::sync::mpsc::Receiver;

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, ItemKind, LayerId, LayerSource, Project, build};
use effectcraft_time::{FrameRate, Tick};
use egui::{Color32, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2, pos2, vec2};
use serde_json::{Value as JsonValue, json};
use strike_character::character::{CharacterError, CharacterProject, CharacterSession};
use strike_character::rig::{PartPose, Rig};

const CANVAS: u32 = 128;
const FRAMES: i64 = 24;

fn starter_cel(head: bool) -> Vec<u8> {
    let mut pixels = vec![0; (CANVAS * CANVAS * 4) as usize];
    for y in 0..CANVAS {
        for x in 0..CANVAS {
            let point = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let (cx, cy, rx, ry) = if head { (64.0, 46.0, 17.0, 18.0) } else { (64.0, 83.0, 24.0, 29.0) };
            let distance = ((point.0 - cx) / rx).powi(2) + ((point.1 - cy) / ry).powi(2);
            let color = if head {
                let eye = (point.1 - 44.0).abs() < 2.0 && ((point.0 - 58.0).abs() < 1.5 || (point.0 - 70.0).abs() < 1.5);
                if eye { [38, 49, 63, 255] } else { [244, 182, 139, 255] }
            } else {
                [77, 164, 190, 255]
            };
            if distance <= 1.0 {
                let index = ((y * CANVAS + x) * 4) as usize;
                pixels[index..index + 4].copy_from_slice(&color);
            }
        }
    }
    pixels
}

fn new_character() -> Result<CharacterProject, CharacterError> {
    let mut project = Project::default();
    let comp = Comp::new(CANVAS, CANVAS, FrameRate::new(12, 1), Tick::from_seconds_f64(2.0));
    let comp_id = project.add_item("Character", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let mut root = build::layer(&mut project, &comp, "Root rig", LayerSource::Null, (CANVAS, CANVAS), None);
    let mut torso = build::layer(&mut project, &comp, "Torso cel", LayerSource::Shape, (CANVAS, CANVAS), None);
    let mut head = build::layer(&mut project, &comp, "Head cel", LayerSource::Shape, (CANVAS, CANVAS), None);
    torso.parent = Some(root.id);
    head.parent = Some(torso.id);
    let parts = [root.id, torso.id, head.id];
    for layer in [&mut root, &mut torso, &mut head] {
        for property in ["anchor", "position"] {
            let value = layer.props.prop_mut(&format!("transform/{property}")).ok_or(CharacterError::Invalid("donor transform is missing"))?;
            value.value = Value::Vec3([f64::from(CANVAS) / 2.0, f64::from(CANVAS) / 2.0, 0.0]);
        }
    }
    project.comp_mut(comp_id).ok_or(CharacterError::Invalid("new composition is missing"))?.layers.extend([root, torso, head]);
    let rig = Rig::validate(&project, comp_id, parts.to_vec())?;
    let mut doc = CharacterProject::new(project, rig)?;
    doc.put_cel(parts[1], 0, CANVAS, CANVAS, starter_cel(false))?;
    doc.put_cel(parts[2], 0, CANVAS, CANVAS, starter_cel(true))?;
    Ok(doc)
}

fn validate_editor_profile(doc: &CharacterProject) -> Result<(), CharacterError> {
    let comp = doc.project.comp(doc.rig.comp).ok_or(CharacterError::Invalid("character composition is missing"))?;
    if comp.width != CANVAS || comp.height != CANVAS || doc.frame_count()? != FRAMES || doc.rig.parts.len() != 3 {
        return Err(CharacterError::Invalid("this editor opens 128 x 128, 24-frame, three-part characters only"));
    }
    if !matches!(comp.layer(doc.rig.parts[0]).map(|part| &part.source), Some(LayerSource::Null))
        || doc.rig.parts[1..].iter().any(|id| !matches!(comp.layer(*id).map(|part| &part.source), Some(LayerSource::Footage { .. })))
        || doc.assets.iter().any(|asset| asset.width != CANVAS || asset.height != CANVAS)
    {
        return Err(CharacterError::Invalid("this editor requires a root rig and two 128 x 128 cel parts"));
    }
    Ok(())
}

fn control_i64(params: &JsonValue, name: &str) -> Result<i64, String> {
    params.get(name).and_then(JsonValue::as_i64).ok_or_else(|| format!("{name} must be an integer"))
}

fn control_f64(params: &JsonValue, name: &str) -> Result<f64, String> {
    let value = params.get(name).and_then(JsonValue::as_f64).ok_or_else(|| format!("{name} must be a number"))?;
    if !value.is_finite() {
        return Err(format!("{name} must be finite"));
    }
    Ok(value)
}

fn control_path(params: &JsonValue) -> Result<&Path, String> {
    let text = params
        .get("path")
        .and_then(JsonValue::as_str)
        .filter(|path| !path.is_empty() && path.len() <= 4096)
        .ok_or("path must be a nonempty string up to 4096 bytes")?;
    Ok(Path::new(text))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| e.to_string())?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!("capture write failed: {error}"));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct PoseFields {
    x: f64,
    y: f64,
    rotation: f64,
    scale: f64,
}

impl Default for PoseFields {
    fn default() -> Self {
        Self { x: f64::from(CANVAS) / 2.0, y: f64::from(CANVAS) / 2.0, rotation: 0.0, scale: 100.0 }
    }
}

struct Stroke {
    part: LayerId,
    frame: i64,
    pixels: Vec<u8>,
    last: Option<(i32, i32)>,
    revision: u64,
}

enum Pending {
    New,
    Open(String),
}

struct CharacterApp {
    session: CharacterSession,
    frame: i64,
    part: LayerId,
    pose: PoseFields,
    color: Color32,
    erase: bool,
    radius: i32,
    stroke: Option<Stroke>,
    preview: Option<(u64, i64, u64, TextureHandle)>,
    generation: u64,
    save_path: String,
    open_path: String,
    export_dir: String,
    dirty: bool,
    pending: Option<Pending>,
    status: String,
    control_rx: Option<Receiver<control::Request>>,
    control_roots: Option<control::Roots>,
}

impl CharacterApp {
    fn new(cc: &eframe::CreationContext<'_>, options: Option<control::Options>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        let mut style = (*cc.egui_ctx.style_of(egui::Theme::Dark)).clone();
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(12.0, 7.0);
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(23, 27, 35);
        style.visuals.window_fill = Color32::from_rgb(30, 35, 44);
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(54, 118, 156);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(45, 73, 91);
        cc.egui_ctx.set_style_of(egui::Theme::Dark, style);
        let mut app = Self::fresh()?;
        if let Some(options) = options {
            app.control_rx = Some(control::start(options.port, options.token, cc.egui_ctx.clone())?);
            app.control_roots = Some(options.roots);
        }
        Ok(app)
    }

    fn fresh() -> Result<Self, CharacterError> {
        let doc = new_character()?;
        let part = doc.rig.parts[1];
        let mut app = Self {
            session: CharacterSession::new(doc)?,
            frame: 0,
            part,
            pose: PoseFields::default(),
            color: Color32::from_rgb(240, 111, 82),
            erase: false,
            radius: 4,
            stroke: None,
            preview: None,
            generation: 0,
            save_path: "character-001.animatestrike".into(),
            open_path: String::new(),
            export_dir: "character-frames-001".into(),
            dirty: false,
            pending: None,
            status: "Draw on a cel, move to another frame, then draw again to split its hold.".into(),
            control_rx: None,
            control_roots: None,
        };
        app.sync_pose();
        Ok(app)
    }

    fn current_pose(&self) -> Option<PartPose> {
        self.session.current.rig.pose_at(&self.session.current.project, self.frame).ok()?.into_iter().find(|pose| pose.part == self.part)
    }

    fn sync_pose(&mut self) {
        if let Some(pose) = self.current_pose() {
            self.pose = PoseFields { x: pose.position[0], y: pose.position[1], rotation: pose.rotation_degrees, scale: pose.scale[0] };
        }
    }

    fn change_frame(&mut self, frame: i64) {
        if frame != self.frame {
            self.finish_stroke();
            self.frame = frame;
            self.sync_pose();
        }
    }

    fn change_part(&mut self, part: LayerId) {
        if part != self.part {
            self.finish_stroke();
            self.part = part;
            self.sync_pose();
        }
    }

    fn set_key_action(&mut self) -> Result<(), CharacterError> {
        let (part, frame, pose) = (self.part, self.frame, self.pose);
        self.session.edit(|doc| {
            doc.set_pose_key(part, frame, "position", Value::Vec3([pose.x, pose.y, 0.0]))?;
            doc.set_pose_key(part, frame, "rotation", Value::Scalar(pose.rotation))?;
            doc.set_pose_key(part, frame, "scale", Value::Vec3([pose.scale; 3]))
        })?;
        self.dirty = true;
        self.generation += 1;
        self.status = format!("Pose key stored at frame {}.", frame + 1);
        Ok(())
    }

    fn set_key(&mut self) {
        if let Err(error) = self.set_key_action() {
            self.status = format!("Pose key rejected: {error}");
        }
    }

    fn start_stroke(&mut self) {
        if self.part == self.session.current.rig.parts[0] {
            self.status = "Select Torso cel or Head cel to paint; Root rig carries pose only.".into();
            return;
        }
        let pixels = match self.session.current.asset_at(self.part, self.frame) {
            Ok(Some(asset)) => asset.rgba.clone(),
            Ok(None) => vec![0; (CANVAS * CANVAS * 4) as usize],
            Err(error) => {
                self.status = format!("Cannot start cel stroke: {error}");
                return;
            }
        };
        self.stroke = Some(Stroke { part: self.part, frame: self.frame, pixels, last: None, revision: 0 });
    }

    fn paint_at(&mut self, x: i32, y: i32) {
        let Some(stroke) = self.stroke.as_mut() else { return };
        let from = stroke.last.unwrap_or((x, y));
        let steps = (x - from.0).abs().max((y - from.1).abs()).max(1);
        let color = self.color.to_array();
        for step in 0..=steps {
            let cx = from.0 + (x - from.0) * step / steps;
            let cy = from.1 + (y - from.1) * step / steps;
            for dy in -self.radius..=self.radius {
                for dx in -self.radius..=self.radius {
                    if dx * dx + dy * dy > self.radius * self.radius {
                        continue;
                    }
                    let (px, py) = (cx + dx, cy + dy);
                    if !(0..CANVAS as i32).contains(&px) || !(0..CANVAS as i32).contains(&py) {
                        continue;
                    }
                    let index = ((py as u32 * CANVAS + px as u32) * 4) as usize;
                    if let Some(pixel) = stroke.pixels.get_mut(index..index + 4) {
                        pixel.copy_from_slice(if self.erase { &[0, 0, 0, 0] } else { &color });
                    }
                }
            }
        }
        stroke.last = Some((x, y));
        stroke.revision += 1;
    }

    fn finish_stroke_action(&mut self) -> Result<(), CharacterError> {
        let Some(stroke) = self.stroke.take() else { return Ok(()) };
        if stroke.last.is_none() {
            return Ok(());
        }
        self.session.edit(|doc| doc.put_cel(stroke.part, stroke.frame, CANVAS, CANVAS, stroke.pixels))?;
        self.dirty = true;
        self.generation += 1;
        self.status = format!("Cel painted at frame {}; earlier held frames are unchanged.", stroke.frame + 1);
        Ok(())
    }

    fn finish_stroke(&mut self) {
        if let Err(error) = self.finish_stroke_action() {
            self.status = format!("Cel stroke rejected: {error}");
        }
    }

    fn undo_action(&mut self) -> bool {
        self.finish_stroke();
        if self.session.undo() {
            self.generation += 1;
            self.sync_pose();
            self.dirty = true;
            self.status = "Undo applied.".into();
            true
        } else {
            false
        }
    }

    fn redo_action(&mut self) -> bool {
        self.finish_stroke();
        if self.session.redo() {
            self.generation += 1;
            self.sync_pose();
            self.dirty = true;
            self.status = "Redo applied.".into();
            true
        } else {
            false
        }
    }

    fn texture(&mut self, ctx: &egui::Context) -> Result<&TextureHandle, CharacterError> {
        let draft = self.stroke.as_ref().map_or(0, |stroke| stroke.revision);
        let changed = self.preview.as_ref().is_none_or(|(generation, frame, rev, _)| (*generation, *frame, *rev) != (self.generation, self.frame, draft));
        if changed {
            let image = if let Some(stroke) = &self.stroke {
                let mut temporary = self.session.current.clone();
                temporary.put_cel(stroke.part, stroke.frame, CANVAS, CANVAS, stroke.pixels.clone())?;
                temporary.render_frame(self.frame)?
            } else {
                self.session.current.render_frame(self.frame)?
            };
            let pixels = image.to_rgba8();
            let color_image = egui::ColorImage::from_rgba_unmultiplied([image.width as usize, image.height as usize], &pixels);
            let texture = ctx.load_texture("character-canvas", color_image, TextureOptions::NEAREST);
            self.preview = Some((self.generation, self.frame, draft, texture));
        }
        self.preview.as_ref().map(|(_, _, _, texture)| texture).ok_or(CharacterError::Invalid("preview texture is unavailable"))
    }

    fn save_action(&mut self, path: &Path) -> Result<(), CharacterError> {
        self.finish_stroke_action()?;
        if path.extension().and_then(|extension| extension.to_str()) != Some("animatestrike") {
            return Err(CharacterError::Invalid("save path must end in .animatestrike"));
        }
        self.session.current.save_new(path)?;
        self.dirty = false;
        self.status = format!("Saved new editable revision: {}", path.display());
        Ok(())
    }

    fn save(&mut self) {
        let path = self.save_path.trim().to_owned();
        if let Err(error) = self.save_action(Path::new(&path)) {
            self.status = format!("Save failed; current document is unchanged: {error}");
        }
    }

    fn open_action(&mut self, path: &Path) -> Result<(), CharacterError> {
        let session = CharacterProject::load(path).and_then(|doc| {
            validate_editor_profile(&doc)?;
            CharacterSession::new(doc)
        })?;
        self.session = session;
        self.frame = 0;
        self.part = self.session.current.rig.parts.get(1).copied().unwrap_or(self.session.current.rig.parts[0]);
        self.stroke = None;
        self.generation += 1;
        self.preview = None;
        self.dirty = false;
        self.sync_pose();
        self.status = format!("Reopened editable character: {}", path.display());
        Ok(())
    }

    fn open(&mut self, path: &str) {
        match self.open_action(Path::new(path)) {
            Ok(()) => {}
            Err(error) => self.status = format!("Open failed; current document is unchanged: {error}"),
        }
    }

    fn export_action(&mut self, dir: &Path) -> Result<i64, CharacterError> {
        self.finish_stroke_action()?;
        let end = self.session.current.frame_count()?;
        self.session.current.export_png_sequence(dir, 0, end)?;
        self.status = format!("Exported {end} PNG frames into {}.", dir.display());
        Ok(end)
    }

    fn export(&mut self) {
        let dir = self.export_dir.trim().to_owned();
        if let Err(error) = self.export_action(Path::new(&dir)) {
            self.status = format!("Export failed: {error}. A newly created folder may contain completed frames; existing folders were preserved.");
        }
    }

    fn selected_part(&self, index: i64) -> Result<LayerId, String> {
        let index = usize::try_from(index).map_err(|_| "part index must be 0, 1 or 2")?;
        self.session.current.rig.parts.get(index).copied().ok_or_else(|| "part index must be 0, 1 or 2".into())
    }

    fn control_result(&mut self, method: &str, params: &JsonValue) -> Result<JsonValue, String> {
        let roots = self.control_roots.clone().ok_or("control is not configured")?;
        match method {
            "state" => {
                let pose = self.current_pose();
                let part = self.session.current.rig.parts.iter().position(|id| *id == self.part);
                let cel_starts: Vec<i64> =
                    self.session.current.exposures.iter().filter(|exposure| exposure.part == self.part).map(|exposure| exposure.frame).collect();
                Ok(
                    json!({"frame": self.frame, "part": part, "dirty": self.dirty, "undo": self.session.can_undo(), "redo": self.session.can_redo(), "cel_starts": cel_starts,
                    "pose": pose.map(|pose| json!({"position": pose.position, "rotation": pose.rotation_degrees, "scale": pose.scale, "world_anchor": pose.world_anchor})), "status": self.status}),
                )
            }
            "part.select" => {
                let part = self.selected_part(control_i64(params, "part")?)?;
                self.change_part(part);
                Ok(json!({"part": self.session.current.rig.parts.iter().position(|id| *id == self.part)}))
            }
            "frame.set" => {
                let frame = control_i64(params, "frame")?;
                if !(0..FRAMES).contains(&frame) {
                    return Err("frame must be 0..23".into());
                }
                self.change_frame(frame);
                Ok(json!({"frame": self.frame}))
            }
            "brush.stroke" => {
                let part = self.selected_part(control_i64(params, "part")?)?;
                if Some(&part) == self.session.current.rig.parts.first() {
                    return Err("root rig is not a paintable cel".into());
                }
                let frame = control_i64(params, "frame")?;
                if !(0..FRAMES).contains(&frame) {
                    return Err("frame must be 0..23".into());
                }
                let radius = control_i64(params, "radius")?;
                if !(1..=12).contains(&radius) {
                    return Err("radius must be 1..12".into());
                }
                let color: [u8; 4] =
                    serde_json::from_value(params.get("color").cloned().ok_or("color must be RGBA8")?).map_err(|_| "color must be four RGBA8 channels")?;
                let points: Vec<[i32; 2]> =
                    serde_json::from_value(params.get("points").cloned().ok_or("points are required")?).map_err(|_| "points must be integer [x,y] pairs")?;
                if points.is_empty() || points.len() > 256 || points.iter().any(|[x, y]| !(0..CANVAS as i32).contains(x) || !(0..CANVAS as i32).contains(y)) {
                    return Err("stroke needs 1..256 points inside the 128 x 128 cel".into());
                }
                let erase = params.get("erase").and_then(JsonValue::as_bool).unwrap_or(false);
                self.finish_stroke_action().map_err(|e| e.to_string())?;
                self.change_part(part);
                self.change_frame(frame);
                self.color = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
                self.radius = radius as i32;
                self.erase = erase;
                self.start_stroke();
                if self.stroke.is_none() {
                    return Err("cannot start cel stroke".into());
                }
                for [x, y] in points {
                    self.paint_at(x, y);
                }
                self.finish_stroke_action().map_err(|e| e.to_string())?;
                Ok(json!({"frame": frame, "part": self.session.current.rig.parts.iter().position(|id| *id == part), "status": self.status}))
            }
            "pose.key" => {
                let part = self.selected_part(control_i64(params, "part")?)?;
                let frame = control_i64(params, "frame")?;
                if !(0..FRAMES).contains(&frame) {
                    return Err("frame must be 0..23".into());
                }
                let pose = PoseFields {
                    x: control_f64(params, "x")?,
                    y: control_f64(params, "y")?,
                    rotation: control_f64(params, "rotation")?,
                    scale: control_f64(params, "scale")?,
                };
                if !(-512.0..=512.0).contains(&pose.x)
                    || !(-512.0..=512.0).contains(&pose.y)
                    || !(-360.0..=360.0).contains(&pose.rotation)
                    || !(1.0..=400.0).contains(&pose.scale)
                {
                    return Err("pose values exceed the editor controls".into());
                }
                self.finish_stroke_action().map_err(|e| e.to_string())?;
                self.change_part(part);
                self.change_frame(frame);
                self.pose = pose;
                self.set_key_action().map_err(|e| e.to_string())?;
                Ok(json!({"frame": frame, "status": self.status}))
            }
            "edit.undo" => {
                if !self.undo_action() {
                    return Err("nothing to undo".into());
                }
                Ok(json!({"status": self.status}))
            }
            "edit.redo" => {
                if !self.redo_action() {
                    return Err("nothing to redo".into());
                }
                Ok(json!({"status": self.status}))
            }
            "file.save" => {
                let path = roots.new_output(control_path(params)?)?;
                self.save_action(&path).map_err(|e| e.to_string())?;
                Ok(json!({"path": path, "status": self.status}))
            }
            "file.open" => {
                self.finish_stroke_action().map_err(|error| format!("cannot finish active stroke before opening: {error}"))?;
                if self.dirty {
                    return Err("save or discard unsaved edits before opening another project".into());
                }
                let path = roots.read_file(control_path(params)?)?;
                self.open_action(&path).map_err(|e| e.to_string())?;
                Ok(json!({"path": path, "status": self.status}))
            }
            "file.export" => {
                let path = roots.new_output(control_path(params)?)?;
                let frames = self.export_action(&path).map_err(|e| format!("{e}; a newly created folder may contain completed frames"))?;
                Ok(json!({"path": path, "frames": frames, "status": self.status}))
            }
            "canvas.capture" => {
                let path = roots.new_output(control_path(params)?)?;
                if path.extension().and_then(|extension| extension.to_str()) != Some("png") {
                    return Err("capture path must end in .png".into());
                }
                self.finish_stroke_action().map_err(|e| e.to_string())?;
                let png = self.session.current.render_png(self.frame).map_err(|e| e.to_string())?;
                write_new_file(&path, &png)?;
                Ok(
                    json!({"path": path, "frame": self.frame, "width": CANVAS, "height": CANVAS, "bytes": png.len(), "kind": "canvas PNG, not a window screenshot"}),
                )
            }
            _ => Err("unknown control method".into()),
        }
    }

    fn dispatch_control(&mut self) {
        let requests: Vec<_> = self.control_rx.as_ref().map(|rx| rx.try_iter().take(8).collect()).unwrap_or_default();
        for request in requests {
            let result = if std::time::Instant::now() > request.deadline {
                Err("expired control request; no action was applied".into())
            } else {
                self.control_result(&request.method, &request.params)
            };
            let reply = match result {
                Ok(value) => json!({"ok": true, "result": value}),
                Err(error) => json!({"ok": false, "error": error}),
            };
            let _ = request.reply.send(reply);
        }
    }

    fn discard_and_continue(&mut self) {
        if let Some(pending) = self.pending.take() {
            match pending {
                Pending::New => match new_character().and_then(CharacterSession::new) {
                    Ok(session) => {
                        self.session = session;
                        self.frame = 0;
                        self.part = self.session.current.rig.parts[1];
                        self.preview = None;
                        self.generation += 1;
                        self.dirty = false;
                        self.stroke = None;
                        self.sync_pose();
                        self.status = "New character ready.".into();
                    }
                    Err(error) => self.status = format!("New character failed: {error}"),
                },
                Pending::Open(path) => self.open(&path),
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("AnimateStrike").color(Color32::from_rgb(119, 205, 232)));
                ui.separator();
                ui.label("Character / Cels + Rig");
                if self.dirty {
                    ui.colored_label(Color32::from_rgb(255, 185, 102), "Unsaved edits");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(self.session.can_redo(), egui::Button::new("Redo")).clicked() {
                        self.redo_action();
                    }
                    if ui.add_enabled(self.session.can_undo(), egui::Button::new("Undo")).clicked() {
                        self.undo_action();
                    }
                    if ui.button("New").clicked() {
                        self.finish_stroke();
                        self.pending = Some(Pending::New);
                        if !self.dirty {
                            self.discard_and_continue();
                        }
                    }
                });
            });
            ui.add_space(6.0);
        });
    }

    fn parts_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("parts").resizable(true).default_size(190.0).show(ui, |ui| {
            ui.heading("PARTS");
            ui.label(egui::RichText::new("Parented character rig").small().weak());
            ui.separator();
            let choices: Vec<(LayerId, String)> = self
                .session
                .current
                .project
                .comp(self.session.current.rig.comp)
                .map(|comp| self.session.current.rig.parts.iter().filter_map(|id| comp.layer(*id).map(|layer| (*id, layer.name.clone()))).collect())
                .unwrap_or_default();
            for (index, (id, name)) in choices.into_iter().enumerate() {
                let label = format!("{}{}", if index == 0 { "● " } else { "  ◇ " }, name);
                if ui.selectable_label(self.part == id, label).clicked() {
                    self.change_part(id);
                }
            }
            ui.add_space(14.0);
            ui.label("Layer order: Head over Torso");
            ui.label(egui::RichText::new("Root moves both cels. Torso moves Head. Cels hold until the next painted frame.").small().weak());
            ui.separator();
            ui.heading("BRUSH");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.erase, false, "Paint");
                ui.selectable_value(&mut self.erase, true, "Erase");
            });
            ui.add(egui::Slider::new(&mut self.radius, 1..=12).text("Radius"));
            ui.color_edit_button_srgba(&mut self.color);
            ui.label(egui::RichText::new("Drag on the canvas. One drag is one undo step.").small().weak());
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("inspector").resizable(true).default_size(270.0).show(ui, |ui| {
            ui.heading("POSE");
            ui.label(egui::RichText::new("Values are keyed at the selected frame").small().weak());
            ui.separator();
            egui::Grid::new("pose_values").num_columns(2).spacing(vec2(12.0, 11.0)).show(ui, |ui| {
                ui.label("Position X");
                ui.add(egui::DragValue::new(&mut self.pose.x).range(-512.0..=512.0).speed(1.0));
                ui.end_row();
                ui.label("Position Y");
                ui.add(egui::DragValue::new(&mut self.pose.y).range(-512.0..=512.0).speed(1.0));
                ui.end_row();
                ui.label("Rotation");
                ui.add(egui::DragValue::new(&mut self.pose.rotation).range(-360.0..=360.0).suffix("°"));
                ui.end_row();
                ui.label("Scale");
                ui.add(egui::DragValue::new(&mut self.pose.scale).range(1.0..=400.0).suffix("%"));
                ui.end_row();
            });
            ui.add_space(8.0);
            if ui.button("Set pose key").clicked() {
                self.finish_stroke();
                self.set_key();
            }
            if let Some(pose) = self.current_pose() {
                ui.label(egui::RichText::new(format!("World pivot  {:.1}, {:.1}", pose.world_anchor[0], pose.world_anchor[1])).small().weak());
            }
            ui.add_space(18.0);
            ui.heading("FILES");
            ui.label("New editable revision");
            ui.text_edit_singleline(&mut self.save_path);
            if ui.button("Save .animatestrike").clicked() {
                self.save();
            }
            ui.add_space(8.0);
            ui.label("Open editable revision");
            ui.text_edit_singleline(&mut self.open_path);
            if ui.button("Open character").clicked() {
                let path = self.open_path.trim().to_string();
                if path.is_empty() {
                    self.status = "Enter a character project path.".into();
                } else {
                    self.finish_stroke();
                    self.pending = Some(Pending::Open(path));
                    if !self.dirty {
                        self.discard_and_continue();
                    }
                }
            }
            ui.add_space(8.0);
            ui.label("New PNG sequence folder");
            ui.text_edit_singleline(&mut self.export_dir);
            if ui.button("Export 24 PNG frames").clicked() {
                self.export();
            }
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("Existing save files and export folders are preserved. Enter a fresh revision name or folder for each output.")
                    .small()
                    .weak(),
            );
        });
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("timeline").resizable(false).show(ui, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.heading("TIMELINE");
                ui.separator();
                ui.label(format!("Frame {:02} / {FRAMES}  ·  12 fps", self.frame + 1));
                if ui.button("◀").clicked() {
                    self.change_frame((self.frame - 1).max(0));
                }
                if ui.button("▶").clicked() {
                    self.change_frame((self.frame + 1).min(FRAMES - 1));
                }
            });
            let mut selected = self.frame;
            if ui.add(egui::Slider::new(&mut selected, 0..=FRAMES - 1).show_value(false)).changed() {
                self.change_frame(selected);
            }
            let exposures: Vec<String> = self.session.current.exposures.iter().filter(|e| e.part == self.part).map(|e| (e.frame + 1).to_string()).collect();
            ui.label(format!("Cel starts on frames: {}", if exposures.is_empty() { "none (rig only)".into() } else { exposures.join(", ") }));
            ui.label(egui::RichText::new(&self.status).color(Color32::from_rgb(170, 215, 229)));
            ui.add_space(6.0);
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Character canvas");
                ui.label(egui::RichText::new("128 × 128  ·  transparent RGBA cels  ·  CPU reference render").small().weak());
                ui.add_space(8.0);
                let side = ui.available_width().min(ui.available_height()).min(640.0).max(128.0);
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
                let checker = side / 16.0;
                for y in 0..16 {
                    for x in 0..16 {
                        let tile = Rect::from_min_size(pos2(rect.left() + x as f32 * checker, rect.top() + y as f32 * checker), Vec2::splat(checker));
                        let gray = if (x + y) % 2 == 0 { 58 } else { 70 };
                        ui.painter().rect_filled(tile, 0.0, Color32::from_gray(gray));
                    }
                }
                match self.texture(&ctx) {
                    Ok(texture) => {
                        ui.painter().image(texture.id(), rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    Err(error) => {
                        self.status = format!("Preview failed: {error}");
                    }
                }
                ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, Color32::from_gray(118)), egui::StrokeKind::Outside);
                if response.drag_started() || response.clicked() {
                    self.start_stroke();
                }
                if self.stroke.is_some() && (response.dragged() || response.drag_started() || response.clicked()) {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let canvas = [
                            f64::from((pointer.x - rect.left()) * CANVAS as f32 / rect.width()),
                            f64::from((pointer.y - rect.top()) * CANVAS as f32 / rect.height()),
                        ];
                        match self.session.current.rig.canvas_to_local(&self.session.current.project, self.part, self.frame, canvas) {
                            Ok(Some(local)) => {
                                let (x, y) = (local[0].floor() as i32, local[1].floor() as i32);
                                if (0..CANVAS as i32).contains(&x) && (0..CANVAS as i32).contains(&y) {
                                    self.paint_at(x, y);
                                }
                            }
                            Ok(None) => self.status = "Cannot paint through a singular character pose.".into(),
                            Err(error) => self.status = format!("Cannot map the brush to this part: {error}"),
                        }
                    }
                }
                if self.stroke.is_some() && !ctx.input(|input| input.pointer.primary_down()) {
                    self.finish_stroke();
                }
            });
        });
    }
}

impl eframe::App for CharacterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.dispatch_control();
        self.top_bar(ui);
        self.parts_panel(ui);
        self.inspector(ui);
        self.timeline(ui);
        self.canvas(ui);
        if self.pending.is_some() {
            egui::Window::new("Unsaved edits").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(&ctx, |ui| {
                ui.label("Discard the current unsaved edits?");
                ui.label("Saved character revisions remain on disk.");
                ui.horizontal(|ui| {
                    if ui.button("Keep editing").clicked() {
                        self.pending = None;
                    }
                    if ui.button("Discard and continue").clicked() {
                        self.discard_and_continue();
                    }
                });
            });
        }
    }
}

fn main() -> eframe::Result {
    let control = match control::options_from_args() {
        Ok(control) => control,
        Err(error) => {
            eprintln!("animatestrike: {error}");
            std::process::exit(2);
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("AnimateStrike").with_inner_size([1180.0, 790.0]).with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("AnimateStrike", options, Box::new(move |cc| CharacterApp::new(cc, control).map(|app| Box::new(app) as Box<dyn eframe::App>)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedural_character_scene_is_editable_and_parented() {
        let mut character = new_character().unwrap();
        validate_editor_profile(&character).unwrap();
        let mut unsupported = character.clone();
        unsupported.project.comp_mut(unsupported.rig.comp).unwrap().width = 256;
        assert!(validate_editor_profile(&unsupported).is_err());
        assert_eq!(character.rig.parts.len(), 3);
        assert_eq!(character.frame_count().unwrap(), FRAMES);
        let head = character.rig.parts[2];
        character.put_cel(head, 7, CANVAS, CANVAS, vec![0; (CANVAS * CANVAS * 4) as usize]).unwrap();
        assert_eq!(character.asset_at(head, 6).unwrap().unwrap().rgba.len(), (CANVAS * CANVAS * 4) as usize);
        assert_eq!(character.exposures.iter().filter(|e| e.part == head).map(|e| e.frame).collect::<Vec<_>>(), [0, 7]);
    }

    #[test]
    fn procedural_scene_paints_saves_reopens_and_exports() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let folder = std::env::temp_dir().join(format!("animatestrike-app-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("character.animatestrike");
        let sequence = folder.join("frames");
        let mut character = new_character().unwrap();
        let torso = character.rig.parts[1];
        let mut cel = vec![0; (CANVAS * CANVAS * 4) as usize];
        let mark = ((64 * CANVAS + 64) * 4) as usize;
        cel[mark..mark + 4].copy_from_slice(&[240, 111, 82, 255]);
        character.put_cel(torso, 0, CANVAS, CANVAS, cel).unwrap();
        character.set_pose_key(torso, 12, "position", Value::Vec3([75.0, 64.0, 0.0])).unwrap();
        let local = character.rig.canvas_to_local(&character.project, torso, 12, [75.0, 64.0]).unwrap().unwrap();
        assert!((local[0] - 64.0).abs() < 0.01 && (local[1] - 64.0).abs() < 0.01);
        character.save_new(&file).unwrap();
        let reopened = CharacterProject::load(&file).unwrap();
        assert_eq!(reopened.rig.parts.len(), 3);
        assert_eq!(reopened.frame_count().unwrap(), 24);
        assert_eq!(&reopened.asset_at(torso, 0).unwrap().unwrap().rgba[mark..mark + 4], &[240, 111, 82, 255]);
        assert!(reopened.save_new(&file).is_err());
        reopened.export_png_sequence(&sequence, 0, 24).unwrap();
        assert_eq!(std::fs::read_dir(&sequence).unwrap().count(), 24);
        assert!(std::fs::metadata(sequence.join("frame_0000.png")).unwrap().len() > 0);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn control_handlers_share_editor_actions_and_preserve_frame_results() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let folder = std::env::temp_dir().join(format!("animatestrike-control-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let mut app = CharacterApp::fresh().unwrap();
        app.control_roots = Some(control::Roots::new(&folder, &folder).unwrap());
        let call = |app: &mut CharacterApp, method: &str, params: JsonValue| app.control_result(method, &params).unwrap();
        let capture = |app: &mut CharacterApp, name: &str| {
            let path = folder.join(name);
            call(app, "canvas.capture", json!({"path": path}));
            std::fs::read(path).unwrap()
        };
        let initial = capture(&mut app, "initial.png");
        call(&mut app, "brush.stroke", json!({"part": 1, "frame": 6, "points": [[25,25],[27,25]], "color": [255,0,0,255], "radius": 2}));
        assert_eq!(call(&mut app, "state", json!({}))["cel_starts"], json!([0, 6]));
        call(&mut app, "frame.set", json!({"frame": 6}));
        let red = capture(&mut app, "red.png");
        assert_ne!(red, initial);
        call(&mut app, "brush.stroke", json!({"part": 1, "frame": 7, "points": [[25,25]], "color": [0,0,255,255], "radius": 2}));
        assert_eq!(call(&mut app, "state", json!({}))["cel_starts"], json!([0, 6, 7]));
        let blue = capture(&mut app, "blue.png");
        assert_ne!(blue, red);
        call(&mut app, "edit.undo", json!({}));
        assert_eq!(capture(&mut app, "undo.png"), red);
        call(&mut app, "edit.redo", json!({}));
        assert_eq!(capture(&mut app, "redo.png"), blue);
        call(&mut app, "pose.key", json!({"part": 0, "frame": 12, "x": 82.0, "y": 64.0, "rotation": 0.0, "scale": 100.0}));
        assert_eq!(call(&mut app, "state", json!({}))["pose"]["position"][0], 82.0);
        call(&mut app, "part.select", json!({"part": 1}));
        call(&mut app, "frame.set", json!({"frame": 12}));
        assert_ne!(capture(&mut app, "posed.png"), initial);
        call(&mut app, "frame.set", json!({"frame": 7}));
        let before_reopen = capture(&mut app, "before-reopen.png");
        let project = folder.join("loop.animatestrike");
        call(&mut app, "file.save", json!({"path": project}));
        assert!(app.control_result("file.save", &json!({"path": project})).is_err());
        call(&mut app, "file.open", json!({"path": project}));
        call(&mut app, "part.select", json!({"part": 1}));
        call(&mut app, "frame.set", json!({"frame": 7}));
        assert_eq!(capture(&mut app, "reopened.png"), before_reopen);
        let sequence = folder.join("frames");
        assert_eq!(call(&mut app, "file.export", json!({"path": sequence}))["frames"], 24);
        assert_eq!(std::fs::read_dir(&sequence).unwrap().count(), 24);
        assert_eq!(std::fs::read(sequence.join("frame_0007.png")).unwrap(), before_reopen);
        assert!(app.control_result("file.export", &json!({"path": sequence})).is_err());
        assert!(app.control_result("canvas.capture", &json!({"path": folder.parent().unwrap().join("outside.png")})).is_err());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn control_open_commits_active_gui_stroke_before_unsaved_check() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let folder = std::env::temp_dir().join(format!("animatestrike-open-stroke-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("saved.animatestrike");
        new_character().unwrap().save_new(&file).unwrap();
        let mut app = CharacterApp::fresh().unwrap();
        app.control_roots = Some(control::Roots::new(&folder, &folder).unwrap());
        let part = app.part;
        let before = app.session.current.asset_at(part, 0).unwrap().unwrap().rgba.clone();
        app.start_stroke();
        app.paint_at(25, 25);
        assert!(!app.dirty, "a GUI stroke is not committed until pointer release");
        assert!(app.stroke.is_some());
        let result = app.control_result("file.open", &json!({"path": file}));
        assert!(result.unwrap_err().contains("unsaved edits"));
        assert!(app.stroke.is_none());
        assert!(app.dirty);
        assert_ne!(app.session.current.asset_at(part, 0).unwrap().unwrap().rgba, before);
        assert!(app.session.can_undo());
        std::fs::remove_dir_all(folder).unwrap();
    }
}
