//! `/figma/font-preview`: the family name drawn in its own font, as SVG.

use std::fmt::Write;

use ttf_parser::{name_id, Face, OutlineBuilder, Tag};

use crate::fonts::{face_name, read_fvar};

struct SvgPath {
    d: String,
    scale: f32,
    x: f32,
    baseline: f32,
}

impl SvgPath {
    fn point(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "{:.2} {:.2} ", self.x + x * self.scale, self.baseline - y * self.scale);
    }
}

impl OutlineBuilder for SvgPath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.d.push('M');
        self.point(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.d.push('L');
        self.point(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.d.push('Q');
        self.point(x1, y1);
        self.point(x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.d.push('C');
        self.point(x1, y1);
        self.point(x2, y2);
        self.point(x, y);
    }
    fn close(&mut self) {
        self.d.push('Z');
    }
}

/// Pick the face (and variable-font named instance) matching `postscript`.
fn select_face<'a>(data: &'a [u8], postscript: &str) -> Option<Face<'a>> {
    let count = ttf_parser::fonts_in_collection(data).unwrap_or(1);
    let faces: Vec<Face> = (0..count).filter_map(|i| Face::parse(data, i).ok()).collect();
    if postscript.is_empty() {
        return faces.into_iter().next();
    }
    let mut first = None;
    for mut face in faces {
        if face_name(&face, &[name_id::POST_SCRIPT_NAME]) == postscript {
            return Some(face);
        }
        if let Some((axes, instances)) = read_fvar(&face) {
            let matching = instances.iter().find(|inst| {
                inst.postscript_name_id.is_some_and(|id| face_name(&face, &[id]) == postscript)
            });
            if let Some(inst) = matching {
                for (axis, &value) in axes.iter().zip(&inst.coordinates) {
                    face.set_variation(Tag::from_bytes(&axis.tag), value);
                }
                return Some(face);
            }
        }
        first.get_or_insert(face);
    }
    first
}

pub fn render(data: &[u8], postscript: &str, text: &str, size_pt: f32) -> Option<String> {
    let face = select_face(data, postscript)?;
    // Figma sends the size in points; render at 96 dpi like the official agent.
    let scale = size_pt / 72.0 * 96.0 / face.units_per_em() as f32;
    let ascent = face.ascender() as f32 * scale;
    let descent = -(face.descender() as f32) * scale;

    let mut path = SvgPath { d: String::new(), scale, x: 0.0, baseline: ascent };
    for ch in text.chars() {
        let Some(glyph) = face.glyph_index(ch) else { continue };
        face.outline_glyph(glyph, &mut path);
        path.x += face.glyph_hor_advance(glyph).unwrap_or(0) as f32 * scale;
    }
    if path.d.is_empty() {
        return None;
    }
    let width = path.x.round().max(1.0);
    let height = (ascent + descent).round().max(1.0);
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><path d="{}"/></svg>"#,
        path.d.trim_end()
    ))
}
