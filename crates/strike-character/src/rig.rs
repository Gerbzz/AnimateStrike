//! A bounded 2D character-part view over an EffectCraft composition.

use std::collections::HashSet;

use effectcraft_geom::Vec3;
use effectcraft_keyframe::Value;
use effectcraft_project::{FootageKind, ItemId, ItemKind, Layer, LayerId, LayerSource, Project};
use effectcraft_render::EvalCtx;
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rig {
    pub comp: ItemId,
    /// Stable EffectCraft layer IDs, in the animator's part order.
    pub parts: Vec<LayerId>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RigError {
    #[error("the character composition does not exist")]
    MissingComp,
    #[error("character canvas must be 1 to 2048 pixels on each side")]
    CanvasBounds,
    #[error("character timeline must be 1 to 60 fps and 1 to 600 whole frames")]
    TimelineBounds,
    #[error("character rig must contain 1 to 64 parts")]
    PartCount,
    #[error("duplicate character part {0:?}")]
    DuplicatePart(LayerId),
    #[error("character part {0:?} is absent from the composition")]
    MissingPart(LayerId),
    #[error("part {0:?} uses a source outside the 2D character subset")]
    UnsupportedSource(LayerId),
    #[error("part {0:?} uses a 3D or nonpositive time stretch")]
    UnsupportedTransform(LayerId),
    #[error("part {0:?} has a parent outside the character rig")]
    MissingParent(LayerId),
    #[error("parent cycle includes part {0:?}")]
    ParentCycle(LayerId),
    #[error("part {0:?} has a missing or invalid transform property")]
    InvalidProperty(LayerId),
    #[error("part {0:?} has a transform key outside its timeline span")]
    InvalidKeyTime(LayerId),
    #[error("frame lies outside the character timeline")]
    FrameBounds,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartPose {
    pub part: LayerId,
    pub anchor: [f64; 3],
    pub position: [f64; 3],
    pub scale: [f64; 3],
    pub rotation_degrees: f64,
    /// Anchor point in canvas coordinates after the donor's parent transforms.
    pub world_anchor: [f64; 3],
}

impl Rig {
    /// Convert a canvas pointer to pixel coordinates in a posed part's cel,
    /// including all parent transforms. A singular transform has no drawable inverse.
    pub fn canvas_to_local(&self, project: &Project, part: LayerId, frame: i64, canvas: [f64; 2]) -> Result<Option<[f64; 2]>, RigError> {
        Self::validate(project, self.comp, self.parts.clone())?;
        let comp = project.comp(self.comp).ok_or(RigError::MissingComp)?;
        if frame < 0 || frame >= comp.frame_rate.frame_at(comp.duration) {
            return Err(RigError::FrameBounds);
        }
        if !canvas.iter().all(|value| value.is_finite()) {
            return Ok(None);
        }
        let layer = comp.layer(part).filter(|_| self.parts.contains(&part)).ok_or(RigError::MissingPart(part))?;
        let ctx = EvalCtx::new(project, self.comp, comp, comp.frame_rate.tick_of(frame));
        let local = ctx.world_matrix(layer).inverse().map(|inverse| inverse.apply(Vec3::from([canvas[0], canvas[1], 0.0])));
        Ok(local.filter(|point| point.x.is_finite() && point.y.is_finite()).map(|point| [point.x, point.y]))
    }

    /// Sample actual donor keyframes and parent transforms at a whole frame.
    pub fn pose_at(&self, project: &Project, frame: i64) -> Result<Vec<PartPose>, RigError> {
        Self::validate(project, self.comp, self.parts.clone())?;
        let comp = project.comp(self.comp).ok_or(RigError::MissingComp)?;
        if frame < 0 || frame >= comp.frame_rate.frame_at(comp.duration) {
            return Err(RigError::FrameBounds);
        }
        let ctx = EvalCtx::new(project, self.comp, comp, comp.frame_rate.tick_of(frame));
        self.parts
            .iter()
            .map(|id| {
                let layer = comp.layer(*id).ok_or(RigError::MissingPart(*id))?;
                let tr = layer.transform().ok_or(RigError::InvalidProperty(*id))?;
                let anchor = ctx.v3(layer, tr, "anchor", [0.0; 3]);
                let position = ctx.position(layer, tr);
                let scale = ctx.v3(layer, tr, "scale", [100.0; 3]);
                let rotation_degrees = ctx.f(layer, tr, "rotation", 0.0);
                let world = ctx.world_matrix(layer).apply(Vec3::from(anchor));
                Ok(PartPose { part: *id, anchor, position, scale, rotation_degrees, world_anchor: [world.x, world.y, world.z] })
            })
            .collect()
    }
    /// Validate a proposed character view before attaching it to an editable project.
    /// The input project is never modified. Unsupported donor layers stay in the
    /// project, but cannot silently enter this rig's part list.
    pub fn validate(project: &Project, comp_id: ItemId, parts: Vec<LayerId>) -> Result<Self, RigError> {
        let comp = project.comp(comp_id).ok_or(RigError::MissingComp)?;
        if !(1..=2048).contains(&comp.width) || !(1..=2048).contains(&comp.height) {
            return Err(RigError::CanvasBounds);
        }
        let rate = comp.frame_rate;
        let frames = rate.frame_at(comp.duration);
        if rate.num <= 0
            || rate.den <= 0
            || i128::from(rate.num) < i128::from(rate.den)
            || i128::from(rate.num) > 60 * i128::from(rate.den)
            || !(1..=600).contains(&frames)
            || rate.tick_of(frames) != comp.duration
        {
            return Err(RigError::TimelineBounds);
        }
        if !(1..=64).contains(&parts.len()) {
            return Err(RigError::PartCount);
        }
        let mut all_layer_ids = HashSet::with_capacity(comp.layers.len());
        for layer in &comp.layers {
            if !all_layer_ids.insert(layer.id) {
                return Err(RigError::DuplicatePart(layer.id));
            }
        }
        let mut ids = HashSet::with_capacity(parts.len());
        for id in &parts {
            if !ids.insert(*id) {
                return Err(RigError::DuplicatePart(*id));
            }
            let layer = comp.layer(*id).ok_or(RigError::MissingPart(*id))?;
            let supported = match &layer.source {
                LayerSource::Shape | LayerSource::Null => true,
                LayerSource::Solid { item } => project.item(*item).is_some_and(|asset| matches!(&asset.kind, ItemKind::Solid(_))),
                LayerSource::Footage { item } => {
                    project.item(*item).is_some_and(|asset| matches!(&asset.kind, ItemKind::Footage(footage) if footage.kind == FootageKind::Still))
                }
                _ => false,
            };
            if !supported {
                return Err(RigError::UnsupportedSource(*id));
            }
            if layer.is_3d() || !layer.stretch.is_finite() || layer.stretch <= 0.0 {
                return Err(RigError::UnsupportedTransform(*id));
            }
            validate_props(layer, comp.duration)?;
        }
        for id in &parts {
            let mut chain = HashSet::new();
            let mut current = Some(*id);
            while let Some(at) = current {
                if !chain.insert(at) {
                    return Err(RigError::ParentCycle(at));
                }
                let layer = comp.layer(at).ok_or(RigError::MissingPart(at))?;
                current = layer.parent;
                if let Some(parent) = current
                    && !ids.contains(&parent)
                {
                    return Err(RigError::MissingParent(at));
                }
            }
        }
        Ok(Self { comp: comp_id, parts })
    }
}

fn validate_props(layer: &Layer, duration: Tick) -> Result<(), RigError> {
    for name in ["anchor", "position", "scale", "rotation"] {
        let Some(property) = layer.props.prop(&format!("transform/{name}")) else {
            return Err(RigError::InvalidProperty(layer.id));
        };
        if property.has_expression() || !valid_value(name, &property.value) {
            return Err(RigError::InvalidProperty(layer.id));
        }
        for key in &property.keys {
            let comp_time = layer.comp_time(key.time);
            if key.time < Tick::ZERO || comp_time < Tick::ZERO || comp_time >= duration || comp_time < layer.in_point || comp_time >= layer.out_point {
                return Err(RigError::InvalidKeyTime(layer.id));
            }
            if !valid_value(name, &key.value) {
                return Err(RigError::InvalidProperty(layer.id));
            }
        }
    }
    Ok(())
}

fn valid_value(name: &str, value: &Value) -> bool {
    match (name, value) {
        ("rotation", Value::Scalar(degrees)) => degrees.is_finite(),
        ("scale", Value::Vec3(scale)) => scale.iter().all(|v| v.is_finite() && *v > 0.0),
        ("anchor" | "position", Value::Vec3(point)) => point.iter().all(|v| v.is_finite()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use effectcraft_color::Label;
    use effectcraft_project::{Comp, ItemKind, LayerSource, Project, build};
    use effectcraft_time::{FrameRate, Tick};

    use super::{Rig, RigError};

    fn two_parts() -> (Project, effectcraft_project::ItemId, [effectcraft_project::LayerId; 2]) {
        let mut project = Project::default();
        let comp = Comp::new(512, 512, FrameRate::new(12, 1), Tick::from_seconds_f64(2.0));
        let comp_id = project.add_item("Character", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
        let root = build::layer(&mut project, &comp, "Root", LayerSource::Null, (100, 100), None);
        let mut head = build::layer(&mut project, &comp, "Head", LayerSource::Shape, (100, 100), None);
        head.parent = Some(root.id);
        let ids = [root.id, head.id];
        project.comp_mut(comp_id).unwrap().layers.extend([root, head]);
        (project, comp_id, ids)
    }

    #[test]
    fn donor_layers_are_valid_parts_without_new_timeline_ids() {
        let (project, comp, ids) = two_parts();
        assert_eq!(Rig::validate(&project, comp, ids.to_vec()).unwrap(), Rig { comp, parts: ids.to_vec() });
    }

    #[test]
    fn parent_cycle_is_rejected_without_mutating_the_project() {
        let (mut project, comp, ids) = two_parts();
        project.comp_mut(comp).unwrap().layer_mut(ids[0]).unwrap().parent = Some(ids[1]);
        let before = project.clone();
        assert!(matches!(Rig::validate(&project, comp, ids.to_vec()), Err(RigError::ParentCycle(_))));
        assert_eq!(project, before);
    }

    #[test]
    fn duplicate_ids_and_parent_outside_the_rig_are_rejected() {
        let (mut project, comp, ids) = two_parts();
        assert_eq!(Rig::validate(&project, comp, vec![ids[0], ids[0]]), Err(RigError::DuplicatePart(ids[0])));
        assert_eq!(Rig::validate(&project, comp, vec![ids[1]]), Err(RigError::MissingParent(ids[1])));
        let duplicate = project.comp(comp).unwrap().layer(ids[1]).unwrap().clone();
        project.comp_mut(comp).unwrap().layers.push(duplicate);
        assert_eq!(Rig::validate(&project, comp, ids.to_vec()), Err(RigError::DuplicatePart(ids[1])));
    }

    #[test]
    fn invalid_key_time_and_nonpositive_scale_are_rejected() {
        let (mut project, comp, ids) = two_parts();
        let head = project.comp_mut(comp).unwrap().layer_mut(ids[1]).unwrap();
        let position = head.props.prop_mut("transform/position").unwrap();
        position.set_animated(true, Tick::ZERO);
        position.set_value_at(Tick::from_seconds_f64(3.0), effectcraft_keyframe::Value::Vec3([4.0, 5.0, 0.0]));
        assert_eq!(Rig::validate(&project, comp, ids.to_vec()), Err(RigError::InvalidKeyTime(ids[1])));
        let head = project.comp_mut(comp).unwrap().layer_mut(ids[1]).unwrap();
        head.props.prop_mut("transform/position").unwrap().keys.clear();
        head.props.prop_mut("transform/scale").unwrap().value = effectcraft_keyframe::Value::Vec3([100.0, 0.0, 100.0]);
        assert_eq!(Rig::validate(&project, comp, ids.to_vec()), Err(RigError::InvalidProperty(ids[1])));
    }
}
