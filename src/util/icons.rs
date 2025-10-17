use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use image::{ImageReader, RgbaImage};
use once_cell::sync::Lazy;
use resvg::render as render_svg_tree;
use tiny_skia::{Pixmap, Transform};
use usvg::{Options as UsvgOptions, Tree as UsvgTree};

static ICON_CACHE: Lazy<Mutex<HashMap<PathBuf, Arc<RgbaImage>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn load_icon(path: &Path) -> Result<Arc<RgbaImage>> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    if let Some(image) = ICON_CACHE
        .lock()
        .expect("icon cache mutex poisoned")
        .get(&canonical)
        .map(Arc::clone)
    {
        return Ok(image);
    }

    let decoded = decode_icon(&canonical)?;
    let image = Arc::new(decoded);
    ICON_CACHE
        .lock()
        .expect("icon cache mutex poisoned")
        .insert(canonical, Arc::clone(&image));

    Ok(image)
}

fn decode_icon(path: &Path) -> Result<RgbaImage> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "svg" => render_svg_icon(path),
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tiff" | "webp" => load_raster_icon(path),
        _ => load_raster_icon(path),
    }
}

fn load_raster_icon(path: &Path) -> Result<RgbaImage> {
    let reader = ImageReader::open(path)
        .with_context(|| format!("failed to open icon at {}", path.display()))?;
    let image = reader
        .with_guessed_format()
        .context("failed to guess icon image format")?
        .decode()
        .with_context(|| format!("failed to decode icon at {}", path.display()))?;
    Ok(image.to_rgba8())
}

fn render_svg_icon(path: &Path) -> Result<RgbaImage> {
    let data =
        fs::read(path).with_context(|| format!("failed to read svg icon at {}", path.display()))?;

    let mut options = UsvgOptions::default();
    options.resources_dir = path.parent().map(|dir| dir.to_path_buf());
    let tree = UsvgTree::from_data(&data, &options)
        .with_context(|| format!("failed to parse svg icon at {}", path.display()))?;

    let size = tree.size().to_int_size();
    let width = size.width().max(1);
    let height = size.height().max(1);

    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| anyhow!("failed to allocate pixmap for icon {}", path.display()))?;

    {
        let mut pixmap_mut = pixmap.as_mut();
        render_svg_tree(&tree, Transform::identity(), &mut pixmap_mut);
    }

    let mut buffer = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for chunk in pixmap.data().chunks_exact(4) {
        let alpha = chunk[3];
        let (red, green, blue) = if alpha == 0 {
            (0, 0, 0)
        } else {
            (
                unpremultiply_component(chunk[0], alpha),
                unpremultiply_component(chunk[1], alpha),
                unpremultiply_component(chunk[2], alpha),
            )
        };
        buffer.extend_from_slice(&[red, green, blue, alpha]);
    }

    RgbaImage::from_vec(width, height, buffer)
        .ok_or_else(|| anyhow!("failed to build rgba image for icon {}", path.display()))
}

fn unpremultiply_component(component: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        0
    } else {
        let value = (component as u32 * 255 + (alpha as u32 / 2)) / alpha as u32;
        value.min(255) as u8
    }
}
