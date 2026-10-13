//! Self-contained character document and bounded edit transactions.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerId, LayerSource, Project};
use serde::{Deserialize, Serialize};

use crate::rig::{Rig, RigError};

const SCHEMA: u32 = 1;
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RGBA_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CelAsset {
    pub item: ItemId,
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8, embedded to keep the character project portable.
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CelExposure {
    pub part: LayerId,
    pub frame: i64,
    pub asset: ItemId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterProject {
    pub schema: u32,
    pub project: Project,
    pub rig: Rig,
    pub assets: Vec<CelAsset>,
    pub exposures: Vec<CelExposure>,
}

#[derive(Debug, thiserror::Error)]
pub enum CharacterError {
    #[error(transparent)]
    Rig(#[from] RigError),
    #[error("unsupported AnimateStrike document schema")]
    Schema,
    #[error("character document exceeds its size limit")]
    FileBounds,
    #[error("invalid character asset or exposure: {0}")]
    Invalid(&'static str),
    #[error("part does not belong to this character")]
    MissingPart,
    #[error("frame lies outside the character timeline")]
    FrameBounds,
    #[error("invalid pose key value")]
    InvalidPose,
    #[error("document encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("donor project is not safely serializable: {0}")]
    Donor(String),
    #[error("file operation failed: {0}")]
    Io(#[from] std::io::Error),
}

impl CharacterProject {
    pub fn new(project: Project, rig: Rig) -> Result<Self, CharacterError> {
        let document = Self { schema: SCHEMA, project, rig, assets: Vec::new(), exposures: Vec::new() };
        document.validate()?;
        Ok(document)
    }

    pub fn frame_count(&self) -> Result<i64, CharacterError> {
        let comp = self.project.comp(self.rig.comp).ok_or(RigError::MissingComp)?;
        Ok(comp.frame_rate.frame_at(comp.duration))
    }

    pub fn validate(&self) -> Result<(), CharacterError> {
        if self.schema != SCHEMA {
            return Err(CharacterError::Schema);
        }
        Rig::validate(&self.project, self.rig.comp, self.rig.parts.clone())?;
        let comp = self.project.comp(self.rig.comp).ok_or(RigError::MissingComp)?;
        if comp.layers.len() != self.rig.parts.len() {
            return Err(CharacterError::Invalid("composition has layers outside the character rig"));
        }
        let mut asset_ids = HashSet::new();
        let mut total = 0usize;
        for asset in &self.assets {
            let size = usize::try_from(asset.width)
                .ok()
                .and_then(|w| usize::try_from(asset.height).ok().and_then(|h| w.checked_mul(h)))
                .and_then(|pixels| pixels.checked_mul(4));
            if !(1..=2048).contains(&asset.width) || !(1..=2048).contains(&asset.height) || size != Some(asset.rgba.len()) || !asset_ids.insert(asset.item) {
                return Err(CharacterError::Invalid("bad cel dimensions, pixels, or duplicate ID"));
            }
            total = total.checked_add(asset.rgba.len()).ok_or(CharacterError::FileBounds)?;
            if total > MAX_RGBA_BYTES {
                return Err(CharacterError::FileBounds);
            }
            if !matches!(self.project.item(asset.item).map(|item| &item.kind), Some(ItemKind::Footage(f)) if f.kind == FootageKind::Still && f.width == asset.width && f.height == asset.height && f.path == format!("embedded://{}", asset.item.0))
            {
                return Err(CharacterError::Invalid("cel's donor footage item is missing or inconsistent"));
            }
        }
        let mut last: HashMap<LayerId, i64> = HashMap::new();
        let frames = self.frame_count()?;
        for exposure in &self.exposures {
            if !self.rig.parts.contains(&exposure.part) || !asset_ids.contains(&exposure.asset) || !(0..frames).contains(&exposure.frame) {
                return Err(CharacterError::Invalid("exposure references a missing part, cel or frame"));
            }
            if let Some(previous) = last.insert(exposure.part, exposure.frame) {
                if previous >= exposure.frame {
                    return Err(CharacterError::Invalid("exposures must be strictly ordered per part"));
                }
            } else if exposure.frame != 0 {
                return Err(CharacterError::Invalid("first cel exposure must start at frame zero"));
            }
        }
        for layer in &comp.layers {
            if let LayerSource::Footage { item } = &layer.source {
                if self.exposures.iter().find(|e| e.part == layer.id && e.frame == 0).map(|e| e.asset) != Some(*item) {
                    return Err(CharacterError::Invalid("footage part has no matching first embedded cel"));
                }
            } else if self.exposures.iter().any(|e| e.part == layer.id) {
                return Err(CharacterError::Invalid("cel exposures belong only to footage parts"));
            }
        }
        Ok(())
    }

    pub fn asset_at(&self, part: LayerId, frame: i64) -> Result<Option<&CelAsset>, CharacterError> {
        if !self.rig.parts.contains(&part) {
            return Err(CharacterError::MissingPart);
        }
        if !(0..self.frame_count()?).contains(&frame) {
            return Err(CharacterError::FrameBounds);
        }
        let item = self.exposures.iter().filter(|e| e.part == part && e.frame <= frame).last().map(|e| e.asset);
        Ok(item.and_then(|id| self.assets.iter().find(|a| a.item == id)))
    }

    /// Replace or add a held cel exposure. Edits commit only after full validation.
    pub fn put_cel(&mut self, part: LayerId, frame: i64, width: u32, height: u32, rgba: Vec<u8>) -> Result<ItemId, CharacterError> {
        if !self.rig.parts.contains(&part) {
            return Err(CharacterError::MissingPart);
        }
        if !(0..self.frame_count()?).contains(&frame) {
            return Err(CharacterError::FrameBounds);
        }
        if frame != 0 && self.asset_at(part, 0)?.is_none() {
            return Err(CharacterError::Invalid("create frame-zero cel first"));
        }
        let mut next = self.clone();
        let mut footage = Footage { path: String::new(), width, height, ..Footage::default() };
        footage.has_video = true;
        let id = next.project.add_item("Embedded cel", Label::Aqua, None, ItemKind::Footage(footage));
        if let Some(item) = next.project.item_mut(id) {
            if let ItemKind::Footage(f) = &mut item.kind {
                f.path = format!("embedded://{}", id.0);
            }
        }
        next.assets.push(CelAsset { item: id, width, height, rgba });
        if let Some(exposure) = next.exposures.iter_mut().find(|e| e.part == part && e.frame == frame) {
            exposure.asset = id;
        } else {
            next.exposures.push(CelExposure { part, frame, asset: id });
            next.exposures.sort_by_key(|e| (e.part.0, e.frame));
        }
        if frame == 0 {
            let layer = next.project.comp_mut(next.rig.comp).and_then(|c| c.layer_mut(part)).ok_or(CharacterError::MissingPart)?;
            layer.source = LayerSource::Footage { item: id };
        }
        let used: HashSet<ItemId> = next.exposures.iter().map(|exposure| exposure.asset).collect();
        let retired: Vec<ItemId> = next.assets.iter().filter(|asset| !used.contains(&asset.item)).map(|asset| asset.item).collect();
        next.assets.retain(|asset| used.contains(&asset.item));
        for item in retired {
            next.project.items.remove(&item);
        }
        next.validate()?;
        *self = next;
        Ok(id)
    }

    /// Set an animator key on the donor property. A first key also retains the frame-zero value.
    pub fn set_pose_key(&mut self, part: LayerId, frame: i64, name: &str, value: Value) -> Result<(), CharacterError> {
        if !self.rig.parts.contains(&part) {
            return Err(CharacterError::MissingPart);
        }
        if !(0..self.frame_count()?).contains(&frame) {
            return Err(CharacterError::FrameBounds);
        }
        let valid = match (name, &value) {
            ("rotation", Value::Scalar(v)) => v.is_finite(),
            ("scale", Value::Vec3(v)) => v.iter().all(|n| n.is_finite() && *n > 0.0),
            ("position" | "anchor", Value::Vec3(v)) => v.iter().all(|n| n.is_finite()),
            _ => false,
        };
        if !valid {
            return Err(CharacterError::InvalidPose);
        }
        let mut next = self.clone();
        let comp = next.project.comp(next.rig.comp).ok_or(RigError::MissingComp)?;
        let time = comp.frame_rate.tick_of(frame);
        let layer = next.project.comp_mut(next.rig.comp).and_then(|c| c.layer_mut(part)).ok_or(CharacterError::MissingPart)?;
        let local = layer.layer_time(time);
        let local_zero = layer.layer_time(effectcraft_time::Tick::ZERO);
        let property = layer.props.prop_mut(&format!("transform/{name}")).ok_or(CharacterError::InvalidPose)?;
        property.set_animated(true, local_zero);
        property.set_value_at(local, value);
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, CharacterError> {
        self.validate()?;
        self.project.to_file_json().map_err(|e| CharacterError::Donor(e.to_string()))?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(CharacterError::FileBounds);
        }
        Self::from_bytes(&bytes)?;
        Ok(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CharacterError> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(CharacterError::FileBounds);
        }
        let mut document: Self = serde_json::from_slice(bytes)?;
        document.project.fix_next_id();
        document.validate()?;
        document.project.to_file_json().map_err(|e| CharacterError::Donor(e.to_string()))?;
        Ok(document)
    }

    pub fn load(path: &Path) -> Result<Self, CharacterError> {
        let file = std::fs::File::open(path)?;
        if file.metadata()?.len() > MAX_FILE_BYTES {
            return Err(CharacterError::FileBounds);
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }

    /// Stage, sync and atomically link a new immutable revision. Existing
    /// paths are never overwritten; a failed write leaves no partial target.
    pub fn save_new(&self, path: &Path) -> Result<(), CharacterError> {
        let bytes = self.to_bytes()?;
        let mut temporary_name = path.as_os_str().to_os_string();
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        temporary_name.push(format!(".{}.{}.tmp", std::process::id(), nonce));
        let temporary = std::path::PathBuf::from(temporary_name);
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&temporary);
            return Err(CharacterError::Io(error));
        }
        drop(file);
        if let Err(error) = std::fs::hard_link(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(CharacterError::Io(error));
        }
        let _ = std::fs::remove_file(&temporary);
        Ok(())
    }
}

/// Bounded in-memory undo/redo for validated character edits. Saved revisions
/// remain separate files, so a crash does not modify the last saved document.
pub struct CharacterSession {
    pub current: CharacterProject,
    undo: VecDeque<CharacterProject>,
    redo: VecDeque<CharacterProject>,
}

impl CharacterSession {
    pub fn new(current: CharacterProject) -> Result<Self, CharacterError> {
        current.validate()?;
        Ok(Self { current, undo: VecDeque::new(), redo: VecDeque::new() })
    }

    pub fn edit<T>(&mut self, change: impl FnOnce(&mut CharacterProject) -> Result<T, CharacterError>) -> Result<T, CharacterError> {
        let mut next = self.current.clone();
        let result = change(&mut next)?;
        next.validate()?;
        if next != self.current {
            self.undo.push_back(std::mem::replace(&mut self.current, next));
            if self.undo.len() > 8 {
                self.undo.pop_front();
            }
            self.redo.clear();
        }
        Ok(result)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        if let Some(previous) = self.undo.pop_back() {
            self.redo.push_back(std::mem::replace(&mut self.current, previous));
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo.pop_back() {
            self.undo.push_back(std::mem::replace(&mut self.current, next));
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use effectcraft_color::Label;
    use effectcraft_keyframe::Value;
    use effectcraft_project::{Comp, ItemKind, LayerId, LayerSource, Project, build};
    use effectcraft_time::{FrameRate, Tick};

    use super::{CharacterError, CharacterProject, CharacterSession};
    use crate::rig::Rig;

    fn fixture() -> (CharacterProject, [LayerId; 2]) {
        let mut project = Project::default();
        let comp = Comp::new(64, 64, FrameRate::new(12, 1), Tick::from_seconds_f64(2.0));
        let comp_id = project.add_item("Character", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
        let mut root = build::layer(&mut project, &comp, "Root", LayerSource::Null, (8, 8), None);
        root.props.prop_mut("transform/anchor").unwrap().value = Value::Vec3([0.0, 0.0, 0.0]);
        root.props.prop_mut("transform/position").unwrap().value = Value::Vec3([0.0, 0.0, 0.0]);
        let mut head = build::layer(&mut project, &comp, "Head", LayerSource::Shape, (8, 8), None);
        head.parent = Some(root.id);
        let ids = [root.id, head.id];
        project.comp_mut(comp_id).unwrap().layers.extend([root, head]);
        let rig = Rig::validate(&project, comp_id, ids.to_vec()).unwrap();
        (CharacterProject::new(project, rig).unwrap(), ids)
    }

    #[test]
    fn pose_keys_interpolate_through_donor_and_parenting() {
        let (mut document, ids) = fixture();
        let original = document.rig.pose_at(&document.project, 0).unwrap()[1].world_anchor;
        document.set_pose_key(ids[0], 12, "position", Value::Vec3([120.0, 0.0, 0.0])).unwrap();
        let halfway = document.rig.pose_at(&document.project, 6).unwrap();
        assert!((halfway[0].position[0] - 60.0).abs() < 0.01);
        assert!((halfway[1].world_anchor[0] - original[0] - 60.0).abs() < 0.01);
    }

    #[test]
    fn held_cels_render_and_survive_native_roundtrip() {
        let (mut document, ids) = fixture();
        let red = [255, 0, 0, 255].repeat(4);
        let blue = [0, 0, 255, 255].repeat(4);
        let first = document.put_cel(ids[1], 0, 2, 2, red).unwrap();
        let second = document.put_cel(ids[1], 7, 2, 2, blue).unwrap();
        assert_eq!(document.asset_at(ids[1], 6).unwrap().unwrap().item, first);
        assert_eq!(document.asset_at(ids[1], 7).unwrap().unwrap().item, second);
        let png = document.render_png(0).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(png, document.render_png(0).unwrap());
        assert!(document.render_frame(0).unwrap().to_rgba8().chunks_exact(4).any(|p| p[0] > p[2] && p[3] > 0));
        assert!(document.render_frame(7).unwrap().to_rgba8().chunks_exact(4).any(|p| p[2] > p[0] && p[3] > 0));
        let bytes = document.to_bytes().unwrap();
        let reopened = CharacterProject::from_bytes(&bytes).unwrap();
        assert_eq!(reopened.to_bytes().unwrap(), bytes);
        assert_eq!(reopened.render_png(7).unwrap(), document.render_png(7).unwrap());

        let path = std::env::temp_dir().join(format!(
            "animatestrike-fixture-{}-{}.animatestrike",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        document.save_new(&path).unwrap();
        assert!(document.save_new(&path).is_err());
        assert_eq!(CharacterProject::load(&path).unwrap().to_bytes().unwrap(), bytes);
        std::fs::remove_file(path).unwrap();

        let directory = std::env::temp_dir().join(format!(
            "animatestrike-sequence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        document.export_png_sequence(&directory, 6, 8).unwrap();
        assert_eq!(std::fs::read(directory.join("frame_0006.png")).unwrap(), document.render_png(6).unwrap());
        assert_eq!(std::fs::read(directory.join("frame_0007.png")).unwrap(), document.render_png(7).unwrap());
        assert!(document.export_png_sequence(&directory, 6, 8).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bad_references_cycles_and_invalid_edits_preserve_source() {
        let (mut document, ids) = fixture();
        let before = document.clone();
        assert!(matches!(document.put_cel(ids[1], 2, 1, 1, vec![0; 4]), Err(CharacterError::Invalid(_))));
        assert!(matches!(document.put_cel(ids[1], 0, 2, 2, vec![0; 3]), Err(CharacterError::Invalid(_))));
        assert!(matches!(document.set_pose_key(ids[0], 0, "scale", Value::Vec3([0.0, 1.0, 1.0])), Err(CharacterError::InvalidPose)));
        assert_eq!(document, before);
        document.put_cel(ids[1], 0, 1, 1, vec![255, 0, 0, 255]).unwrap();
        let mut missing = document.clone();
        missing.assets.clear();
        assert!(matches!(missing.validate(), Err(CharacterError::Invalid(_))));
        let mut cyclic = document.clone();
        cyclic.project.comp_mut(cyclic.rig.comp).unwrap().layer_mut(ids[0]).unwrap().parent = Some(ids[1]);
        assert!(matches!(cyclic.validate(), Err(CharacterError::Rig(_))));
        assert!(CharacterProject::from_bytes(b"{broken").is_err());
        assert!(matches!(CharacterProject::from_bytes(&vec![0; 32 * 1024 * 1024 + 1]), Err(CharacterError::FileBounds)));
    }

    #[test]
    fn undo_redo_restores_cel_split_and_failed_edit_is_not_recorded() {
        let (mut document, ids) = fixture();
        document.put_cel(ids[1], 0, 1, 1, vec![255, 0, 0, 255]).unwrap();
        let original = document.clone();
        let mut session = CharacterSession::new(document).unwrap();
        assert!(session.edit(|doc| doc.put_cel(ids[1], 7, 1, 1, vec![0, 0, 255, 255])).is_ok());
        assert_ne!(session.current.asset_at(ids[1], 0).unwrap().unwrap().item, session.current.asset_at(ids[1], 7).unwrap().unwrap().item);
        assert!(session.edit(|doc| doc.put_cel(ids[1], 8, 2, 2, vec![0; 3])).is_err());
        assert!(session.undo());
        assert_eq!(session.current, original);
        assert!(!session.undo());
        assert!(session.redo());
        assert_ne!(session.current, original);
    }
}
