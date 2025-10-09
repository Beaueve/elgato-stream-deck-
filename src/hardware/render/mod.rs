mod font;

use anyhow::{Context, Result};
use elgato_streamdeck::StreamDeck;
use elgato_streamdeck::images::ImageRect;
use image::{DynamicImage, Rgb, RgbImage};

use crate::hardware::backend::EncoderDisplay;

const SEGMENT_WIDTH: u32 = 200;
const SEGMENT_HEIGHT: u32 = 100;
const SEGMENT_MARGIN: u32 = 12;
const PROGRESS_HEIGHT: u32 = 12;
const PROGRESS_MARGIN: u32 = 10;
const BACKGROUND: [u8; 3] = [8, 10, 18];
const TITLE_COLOR: [u8; 3] = [180, 190, 210];
const VALUE_COLOR: [u8; 3] = [235, 240, 255];
const STATUS_COLOR: [u8; 3] = [120, 210, 255];
const PLACEHOLDER_COLOR: [u8; 3] = [80, 80, 92];
const PROGRESS_BG: [u8; 3] = [30, 35, 45];
const PROGRESS_FG: [u8; 3] = [0, 180, 120];
const BORDER_COLOR: [u8; 3] = [50, 55, 65];

pub fn flush_strip(deck: &StreamDeck, displays: &[Option<EncoderDisplay>; 4]) -> Result<()> {
    let image = compose_strip(displays)?;
    deck.write_lcd(0, 0, &image)
        .context("failed to push LCD strip image")
}

fn compose_strip(displays: &[Option<EncoderDisplay>; 4]) -> Result<ImageRect> {
    let width = SEGMENT_WIDTH * displays.len() as u32;
    let mut canvas = RgbImage::from_pixel(width, SEGMENT_HEIGHT, Rgb(BACKGROUND));

    for (index, display) in displays.iter().enumerate() {
        let segment = render_segment(display);
        overlay_segment(&mut canvas, &segment, index as u32 * SEGMENT_WIDTH);
    }

    let dynamic = DynamicImage::ImageRgb8(canvas);
    ImageRect::from_image(dynamic).context("failed to encode LCD segment into JPEG")
}

fn render_segment(display: &Option<EncoderDisplay>) -> RgbImage {
    let mut segment = RgbImage::from_pixel(SEGMENT_WIDTH, SEGMENT_HEIGHT, Rgb(BACKGROUND));
    draw_border(&mut segment);

    if let Some(data) = display {
        draw_title(&mut segment, &data.title);
        draw_value(&mut segment, &data.value, data.status.is_some());

        if let Some(status) = &data.status {
            draw_status(&mut segment, status);
        }

        if let Some(progress) = data.progress {
            draw_progress(&mut segment, progress, data.progress_color);
        }
    } else {
        font::draw_text(
            &mut segment,
            "EMPTY",
            SEGMENT_MARGIN,
            SEGMENT_MARGIN,
            2,
            PLACEHOLDER_COLOR,
        );
    }

    segment
}

fn overlay_segment(canvas: &mut RgbImage, segment: &RgbImage, offset_x: u32) {
    for y in 0..SEGMENT_HEIGHT.min(canvas.height()) {
        for x in 0..SEGMENT_WIDTH.min(canvas.width().saturating_sub(offset_x)) {
            let pixel = segment.get_pixel(x, y);
            canvas.put_pixel(offset_x + x, y, *pixel);
        }
    }
}

fn draw_border(segment: &mut RgbImage) {
    let width = segment.width();
    let height = segment.height();

    for x in 0..width {
        segment.put_pixel(x, 0, Rgb(BORDER_COLOR));
        segment.put_pixel(x, height - 1, Rgb(BORDER_COLOR));
    }
    for y in 0..height {
        segment.put_pixel(0, y, Rgb(BORDER_COLOR));
        segment.put_pixel(width - 1, y, Rgb(BORDER_COLOR));
    }
}

fn draw_title(segment: &mut RgbImage, title: &str) {
    let text = title.to_ascii_uppercase();
    font::draw_text(
        segment,
        &text,
        SEGMENT_MARGIN,
        SEGMENT_MARGIN,
        2,
        TITLE_COLOR,
    );
}

fn draw_value(segment: &mut RgbImage, value: &str, has_status: bool) {
    let scale = 4;
    let (text_width, text_height) = font::measure_text(value, scale);
    let mut y_center = (SEGMENT_HEIGHT / 2).saturating_sub(text_height / 2);
    if has_status {
        y_center = y_center.saturating_sub(6);
    }
    let x = ((SEGMENT_WIDTH - text_width) / 2).min(SEGMENT_WIDTH.saturating_sub(text_width));
    font::draw_text(
        segment,
        value,
        x,
        y_center.max(SEGMENT_MARGIN),
        scale,
        VALUE_COLOR,
    );
}

fn draw_status(segment: &mut RgbImage, status: &str) {
    let text = status.to_ascii_uppercase();
    let scale = 2;
    let (text_width, text_height) = font::measure_text(&text, scale);
    let x = ((SEGMENT_WIDTH - text_width) / 2).min(SEGMENT_WIDTH.saturating_sub(text_width));
    let y = SEGMENT_HEIGHT.saturating_sub(PROGRESS_MARGIN + PROGRESS_HEIGHT + text_height + 4);
    font::draw_text(segment, &text, x, y, scale, STATUS_COLOR);
}

fn draw_progress(segment: &mut RgbImage, mut progress: f32, color: Option<[u8; 3]>) {
    progress = progress.clamp(0.0, 1.0);
    let width = SEGMENT_WIDTH.saturating_sub(PROGRESS_MARGIN * 2);
    let x0 = PROGRESS_MARGIN;
    let y0 = SEGMENT_HEIGHT.saturating_sub(PROGRESS_MARGIN + PROGRESS_HEIGHT);
    let fg = color.unwrap_or(PROGRESS_FG);

    for y in 0..PROGRESS_HEIGHT {
        for x in 0..width {
            segment.put_pixel(x0 + x, y0 + y, Rgb(PROGRESS_BG));
        }
    }

    let filled = (progress * width as f32).round() as u32;
    for y in 0..PROGRESS_HEIGHT {
        for x in 0..filled {
            segment.put_pixel(x0 + x, y0 + y, Rgb(fg));
        }
    }
}
