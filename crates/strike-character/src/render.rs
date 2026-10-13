//! Deterministic CPU rendering of the bounded character subset.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use effectcraft_project::{Footage, ItemId, LayerSource};
use effectcraft_raster::Image;
use effectcraft_render::{FootageSource, RenderOpts, Renderer};
use effectcraft_time::Tick;
use image::ImageEncoder;

use crate::character::{CharacterError, CharacterProject};

struct EmbeddedSource(HashMap<ItemId, Arc<Image>>);

impl FootageSource for EmbeddedSource {
    fn frame(&self, item: ItemId, _footage: &Footage, _time: Tick) -> Option<Arc<Image>> {
        self.0.get(&item).cloned()
    }
}

impl CharacterProject {
    /// Evaluate the selected held cels through EffectCraft's actual CPU compositor.
    pub fn render_frame(&self, frame: i64) -> Result<Image, CharacterError> {
        self.validate()?;
        if !(0..self.frame_count()?).contains(&frame) {
            return Err(CharacterError::FrameBounds);
        }
        let mut project = self.project.clone();
        let selections =
            self.rig.parts.iter().map(|part| Ok((*part, self.asset_at(*part, frame)?.map(|asset| asset.item)))).collect::<Result<Vec<_>, CharacterError>>()?;
        let comp = project.comp_mut(self.rig.comp).ok_or(crate::rig::RigError::MissingComp)?;
        for (part, asset) in selections {
            if let Some(asset) = asset {
                let layer = comp.layer_mut(part).ok_or(CharacterError::MissingPart)?;
                layer.source = LayerSource::Footage { item: asset };
            }
        }
        let source = EmbeddedSource(self.assets.iter().map(|a| (a.item, Arc::new(Image::from_rgba8(a.width, a.height, &a.rgba)))).collect());
        let time = comp.frame_rate.tick_of(frame);
        let opts = RenderOpts { motion_blur: false, ..RenderOpts::default() };
        Ok(Renderer::new(&project, &source, opts).comp_frame(self.rig.comp, time))
    }

    /// Encodes one preview/export frame as PNG with straight alpha.
    pub fn render_png(&self, frame: i64) -> Result<Vec<u8>, CharacterError> {
        let image = self.render_frame(frame)?;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&image.to_rgba8(), image.width, image.height, image::ColorType::Rgba8.into())
            .map_err(|_| CharacterError::Invalid("PNG encoding failed"))?;
        Ok(png)
    }

    /// Export at most 120 frames into a newly created directory. Failure leaves
    /// completed frames for inspection, and the caller may choose another path.
    pub fn export_png_sequence(&self, directory: &Path, start: i64, end: i64) -> Result<(), CharacterError> {
        let frames = self.frame_count()?;
        if start < 0 || end <= start || end > frames || end - start > 120 {
            return Err(CharacterError::FrameBounds);
        }
        self.validate()?;
        std::fs::create_dir(directory)?;
        for frame in start..end {
            let png = self.render_png(frame)?;
            let path = directory.join(format!("frame_{frame:04}.png"));
            let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
            file.write_all(&png)?;
            file.sync_all()?;
        }
        Ok(())
    }
}
