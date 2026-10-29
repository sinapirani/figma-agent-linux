//! Font discovery and the `FontInfo` records Figma expects from `/figma/font-files`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use ttf_parser::{name_id, Face, Tag};

const FONT_EXTENSIONS: [&str; 5] = ["ttf", "otf", "ttc", "otc", "woff"];
/// Upper bound for a decoded WOFF font, so a malformed file cannot exhaust memory.
const MAX_WOFF_SIZE: usize = 256 << 20;
/// Minimum time between rescans triggered by requests for unknown files.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);

fn be_u32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// Rebuild a plain sfnt (TrueType/OpenType) font from WOFF 1.0 data.
fn decode_woff(woff: &[u8]) -> Option<Vec<u8>> {
    let flavor = be_u32(woff, 4)?;
    let num_tables = be_u16(woff, 12)? as usize;
    let mut tables = Vec::with_capacity(num_tables);
    let mut total = 0usize;
    for i in 0..num_tables {
        let at = 44 + i * 20;
        let (tag, offset, comp_len, orig_len, checksum) = (
            be_u32(woff, at)?,
            be_u32(woff, at + 4)? as usize,
            be_u32(woff, at + 8)? as usize,
            be_u32(woff, at + 12)? as usize,
            be_u32(woff, at + 16)?,
        );
        total = total.checked_add(orig_len).filter(|&t| t <= MAX_WOFF_SIZE)?;
        let raw = woff.get(offset..offset.checked_add(comp_len)?)?;
        let data = if comp_len < orig_len {
            miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(raw, orig_len).ok()?
        } else {
            raw.to_vec()
        };
        if data.len() != orig_len {
            return None;
        }
        tables.push((tag, checksum, data));
    }

    let entry_selector = (num_tables.max(1) as u32).ilog2() as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let mut out = Vec::new();
    out.extend(flavor.to_be_bytes());
    out.extend((num_tables as u16).to_be_bytes());
    out.extend(search_range.to_be_bytes());
    out.extend(entry_selector.to_be_bytes());
    out.extend((num_tables as u16 * 16 - search_range).to_be_bytes());
    let mut offset = 12 + num_tables * 16;
    for (tag, checksum, data) in &tables {
        out.extend(tag.to_be_bytes());
        out.extend(checksum.to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        offset += data.len().next_multiple_of(4);
    }
    for (_, _, data) in &tables {
        out.extend(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    Some(out)
}

/// Read a font file, transparently decoding WOFF (sometimes shipped with a .ttf name).
pub fn load(path: &str) -> std::io::Result<Vec<u8>> {
    let data = std::fs::read(path)?;
    if data.starts_with(b"wOFF") {
        return decode_woff(&data)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid WOFF data"));
    }
    Ok(data)
}

/// MIME type of decoded font data.
pub fn content_type(data: &[u8]) -> &'static str {
    match data.get(..4) {
        Some(b"OTTO") => "font/otf",
        Some(b"ttcf") => "font/collection",
        _ => "font/ttf",
    }
}

fn has_font_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| FONT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Standard font folders (covers systems without fontconfig) plus `FIGMA_AGENT_FONT_DIRS`.
fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let mut dirs = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
        data_home.join("fonts"),
        home.join(".fonts"),
    ];
    if let Some(extra) = std::env::var_os("FIGMA_AGENT_FONT_DIRS") {
        for dir in std::env::split_paths(&extra).filter(|d| !d.as_os_str().is_empty()) {
            dirs.push(match dir.strip_prefix("~") {
                Ok(rest) => home.join(rest),
                Err(_) => dir,
            });
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

/// System fonts via fontconfig plus everything under the font folders.
fn list_font_paths() -> HashSet<String> {
    let mut paths = HashSet::new();
    if let Ok(out) = Command::new("fc-list").args(["--format", "%{file}\n"]).output() {
        paths.extend(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|p| has_font_extension(Path::new(p)))
                .map(String::from),
        );
    }
    for dir in font_dirs() {
        for entry in walkdir::WalkDir::new(dir).follow_links(true).into_iter().flatten() {
            if entry.file_type().is_file() && has_font_extension(entry.path()) {
                // JSON object keys must be strings; skip the rare non-UTF-8 path.
                if let Some(p) = entry.path().to_str() {
                    paths.insert(p.to_string());
                }
            }
        }
    }
    paths
}

/// A name-table string, preferring US English, trying each id in order.
pub fn face_name(face: &Face, ids: &[u16]) -> String {
    for &id in ids {
        let mut fallback = None;
        for name in face.names().into_iter().filter(|n| n.name_id == id) {
            if let Some(s) = name.to_string().filter(|s| !s.is_empty()) {
                if name.language_id == 0x0409 {
                    return s;
                }
                fallback.get_or_insert(s);
            }
        }
        if let Some(s) = fallback {
            return s;
        }
    }
    String::new()
}

pub struct Axis {
    pub tag: [u8; 4],
    pub min: f32,
    pub default: f32,
    pub max: f32,
    pub hidden: bool,
    pub name_id: u16,
}

pub struct Instance {
    pub subfamily_name_id: u16,
    pub postscript_name_id: Option<u16>,
    pub coordinates: Vec<f32>,
}

fn be_u16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn be_fixed(d: &[u8], at: usize) -> Option<f32> {
    Some(i32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?) as f32 / 65536.0)
}

/// Parse the `fvar` table (axes and named instances) straight from its bytes.
pub fn read_fvar(face: &Face) -> Option<(Vec<Axis>, Vec<Instance>)> {
    let d = face.raw_face().table(Tag::from_bytes(b"fvar"))?;
    let axes_offset = be_u16(d, 4)? as usize;
    let axis_count = be_u16(d, 8)? as usize;
    let axis_size = be_u16(d, 10)? as usize;
    let instance_count = be_u16(d, 12)? as usize;
    let instance_size = be_u16(d, 14)? as usize;

    let mut axes = Vec::with_capacity(axis_count);
    for i in 0..axis_count {
        let at = axes_offset + i * axis_size;
        axes.push(Axis {
            tag: d.get(at..at + 4)?.try_into().ok()?,
            min: be_fixed(d, at + 4)?,
            default: be_fixed(d, at + 8)?,
            max: be_fixed(d, at + 12)?,
            hidden: be_u16(d, at + 16)? & 0x1 != 0,
            name_id: be_u16(d, at + 18)?,
        });
    }

    let first_instance = axes_offset + axis_count * axis_size;
    let has_postscript_id = instance_size >= axis_count * 4 + 6;
    let mut instances = Vec::with_capacity(instance_count);
    for i in 0..instance_count {
        let at = first_instance + i * instance_size;
        let coordinates = (0..axis_count)
            .map(|a| be_fixed(d, at + 4 + a * 4))
            .collect::<Option<Vec<_>>>()?;
        instances.push(Instance {
            subfamily_name_id: be_u16(d, at)?,
            postscript_name_id: if has_postscript_id {
                be_u16(d, at + 4 + axis_count * 4).filter(|&id| id != 0xFFFF)
            } else {
                None
            },
            coordinates,
        });
    }
    Some((axes, instances))
}

/// Italic/oblique from the style flags only (OS/2 fsSelection bits 0 and 9, head macStyle bit 1),
/// like fontconfig; a non-zero post.italicAngle alone does not make an upright face italic.
fn is_italic(face: &Face) -> bool {
    let raw = face.raw_face();
    let fs_selection = raw.table(Tag::from_bytes(b"OS/2")).and_then(|d| be_u16(d, 62)).unwrap_or(0);
    let mac_style = raw.table(Tag::from_bytes(b"head")).and_then(|d| be_u16(d, 44)).unwrap_or(0);
    fs_selection & 0x201 != 0 || mac_style & 0x2 != 0
}

fn width_class(percent: f32) -> u16 {
    const STEPS: [f32; 9] = [50.0, 62.5, 75.0, 87.5, 100.0, 112.5, 125.0, 150.0, 200.0];
    let nearest = (0..9)
        .min_by(|&a, &b| (STEPS[a] - percent).abs().total_cmp(&(STEPS[b] - percent).abs()))
        .unwrap_or(4);
    nearest as u16 + 1
}

/// One `FontInfo` per face, or one per named instance for variable fonts.
fn describe_face(face: &Face, modified_at: u64) -> Vec<Value> {
    let mut base = json!({
        "family": face_name(face, &[name_id::TYPOGRAPHIC_FAMILY, name_id::FAMILY]),
        "style": face_name(face, &[name_id::TYPOGRAPHIC_SUBFAMILY, name_id::SUBFAMILY]),
        "postscript": face_name(face, &[name_id::POST_SCRIPT_NAME]),
        "weight": face.weight().to_number(),
        "stretch": face.width().to_number(),
        "italic": is_italic(face),
        "modified_at": modified_at,
        "user_installed": true,
    });
    let Some((axes, instances)) = read_fvar(face).filter(|(axes, _)| !axes.is_empty()) else {
        return vec![base];
    };

    let axis_json = |values: Option<&[f32]>| -> Value {
        axes.iter()
            .enumerate()
            .map(|(i, a)| {
                let name = face_name(face, &[a.name_id]);
                let tag = String::from_utf8_lossy(&a.tag).into_owned();
                json!({
                    "tag": tag,
                    "name": if name.is_empty() { tag.clone() } else { name },
                    "value": values.map_or(a.default, |v| v[i]),
                    "min": a.min,
                    "max": a.max,
                    "default": a.default,
                    "hidden": a.hidden,
                })
            })
            .collect()
    };
    base["variationAxes"] = axis_json(None);
    if instances.is_empty() {
        return vec![base];
    }

    instances
        .iter()
        .map(|inst| {
            let mut entry = base.clone();
            entry["variationAxes"] = axis_json(Some(&inst.coordinates));
            let style = face_name(face, &[inst.subfamily_name_id]);
            if !style.is_empty() {
                entry["style"] = style.into();
            }
            if let Some(id) = inst.postscript_name_id {
                let ps = face_name(face, &[id]);
                if !ps.is_empty() {
                    entry["postscript"] = ps.into();
                }
            }
            let mut slanted = None;
            for (axis, &value) in axes.iter().zip(&inst.coordinates) {
                match &axis.tag {
                    b"wght" => entry["weight"] = (value.round() as u16).into(),
                    b"wdth" => entry["stretch"] = width_class(value).into(),
                    b"ital" | b"slnt" => slanted = Some(slanted.unwrap_or(false) || value != 0.0),
                    _ => {}
                }
            }
            if let Some(italic) = slanted {
                entry["italic"] = italic.into();
            }
            entry
        })
        .collect()
}

/// The `FontInfo` records for every face in a file, or why none could be read.
pub fn describe_file(path: &str, modified_at: u64) -> Result<Vec<Value>, String> {
    let data = load(path).map_err(|e| e.to_string())?;
    let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
    let mut faces = Vec::new();
    let mut error = None;
    for i in 0..count {
        match Face::parse(&data, i) {
            Ok(face) => faces.extend(describe_face(&face, modified_at)),
            Err(e) => error = Some(format!("face {i}: {e}")),
        }
    }
    match error {
        Some(e) if faces.is_empty() => Err(e),
        _ => Ok(faces),
    }
}

/// Path -> faces, rescanned on demand and cached by file mtime.
#[derive(Default)]
pub struct FontIndex {
    files: Mutex<HashMap<String, (u64, Vec<Value>)>>,
    last_scan: Mutex<Option<Instant>>,
}

impl FontIndex {
    pub fn scan(&self) -> Map<String, Value> {
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        *self.last_scan.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        let mut fresh = HashMap::new();
        for path in list_font_paths() {
            let Some(mtime) = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
            else {
                continue;
            };
            let entry = match files.remove(&path) {
                Some(cached) if cached.0 == mtime => cached,
                _ => (mtime, describe_file(&path, mtime).unwrap_or_default()),
            };
            // Malformed or unsupported files are skipped.
            if !entry.1.is_empty() {
                fresh.insert(path, entry);
            }
        }
        *files = fresh;
        files.iter().map(|(path, (_, faces))| (path.clone(), Value::Array(faces.clone()))).collect()
    }

    /// Rescan unless the last scan was recent, so requests for unknown files cannot keep the agent busy.
    pub fn rescan_if_stale(&self) {
        let last = *self.last_scan.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_none_or(|t| t.elapsed() >= RESCAN_INTERVAL) {
            self.scan();
        }
    }

    /// Only files we indexed ourselves are ever served.
    pub fn contains(&self, path: &str) -> bool {
        self.files.lock().unwrap_or_else(|e| e.into_inner()).contains_key(path)
    }
}
