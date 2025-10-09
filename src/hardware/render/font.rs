use image::{Rgb, RgbImage};

const GLYPH_WIDTH: u32 = 5;
const GLYPH_HEIGHT: u32 = 7;

pub fn measure_text(text: &str, scale: u32) -> (u32, u32) {
    if scale == 0 {
        return (0, 0);
    }

    let mut width = 0;
    let mut drawn = false;

    for ch in text.chars() {
        if ch == ' ' {
            width += scale * 3;
            continue;
        }

        if glyph_for(ch).is_some() {
            if drawn {
                width += scale; // inter-character spacing
            }
            width += GLYPH_WIDTH * scale;
            drawn = true;
        }
    }

    let height = if drawn { GLYPH_HEIGHT * scale } else { 0 };
    (width, height)
}

pub fn draw_text(image: &mut RgbImage, text: &str, mut x: u32, y: u32, scale: u32, color: [u8; 3]) {
    if scale == 0 {
        return;
    }

    for raw_ch in text.chars() {
        let ch = raw_ch.to_ascii_uppercase();
        if ch == ' ' {
            x = x.saturating_add(scale * 3);
            continue;
        }

        let glyph = match glyph_for(ch) {
            Some(g) => g,
            None => {
                x = x.saturating_add(scale * 2);
                continue;
            }
        };

        draw_glyph(image, glyph, x, y, scale, color);
        x = x.saturating_add(GLYPH_WIDTH * scale).saturating_add(scale);
    }
}

fn draw_glyph(
    image: &mut RgbImage,
    glyph: &[&str; GLYPH_HEIGHT as usize],
    origin_x: u32,
    origin_y: u32,
    scale: u32,
    color: [u8; 3],
) {
    for (row_idx, row) in glyph.iter().enumerate() {
        for (col_idx, cell) in row.chars().enumerate() {
            if cell != '#' {
                continue;
            }
            let x0 = origin_x + col_idx as u32 * scale;
            let y0 = origin_y + row_idx as u32 * scale;

            for dy in 0..scale {
                for dx in 0..scale {
                    let x = x0 + dx;
                    let y = y0 + dy;
                    if x < image.width() && y < image.height() {
                        image.put_pixel(x, y, Rgb(color));
                    }
                }
            }
        }
    }
}

fn glyph_for(ch: char) -> Option<&'static [&'static str; GLYPH_HEIGHT as usize]> {
    match ch {
        '0' => Some(&DIGIT_0),
        '1' => Some(&DIGIT_1),
        '2' => Some(&DIGIT_2),
        '3' => Some(&DIGIT_3),
        '4' => Some(&DIGIT_4),
        '5' => Some(&DIGIT_5),
        '6' => Some(&DIGIT_6),
        '7' => Some(&DIGIT_7),
        '8' => Some(&DIGIT_8),
        '9' => Some(&DIGIT_9),
        'A' => Some(&LETTER_A),
        'B' => Some(&LETTER_B),
        'C' => Some(&LETTER_C),
        'D' => Some(&LETTER_D),
        'E' => Some(&LETTER_E),
        'F' => Some(&LETTER_F),
        'G' => Some(&LETTER_G),
        'H' => Some(&LETTER_H),
        'I' => Some(&LETTER_I),
        'L' => Some(&LETTER_L),
        'M' => Some(&LETTER_M),
        'N' => Some(&LETTER_N),
        'O' => Some(&LETTER_O),
        'P' => Some(&LETTER_P),
        'R' => Some(&LETTER_R),
        'S' => Some(&LETTER_S),
        'T' => Some(&LETTER_T),
        'U' => Some(&LETTER_U),
        'V' => Some(&LETTER_V),
        ':' => Some(&GLYPH_COLON),
        '%' => Some(&GLYPH_PERCENT),
        '-' => Some(&GLYPH_DASH),
        _ => None,
    }
}

macro_rules! glyph {
    ($($line:literal),+ $(,)?) => {
        [$( $line ),+]
    };
}

const DIGIT_0: [&str; 7] = glyph![
    " ### ", "#   #", "#  ##", "# # #", "##  #", "#   #", " ### ",
];

const DIGIT_1: [&str; 7] = glyph![
    "  #  ", " ##  ", "# #  ", "  #  ", "  #  ", "  #  ", "#####",
];

const DIGIT_2: [&str; 7] = glyph![
    " ### ", "#   #", "    #", "   # ", "  #  ", " #   ", "#####",
];

const DIGIT_3: [&str; 7] = glyph![
    " ### ", "#   #", "    #", " ### ", "    #", "#   #", " ### ",
];

const DIGIT_4: [&str; 7] = glyph![
    "#   #", "#   #", "#   #", "#####", "    #", "    #", "    #",
];

const DIGIT_5: [&str; 7] = glyph![
    "#####", "#    ", "#    ", "#### ", "    #", "#   #", " ### ",
];

const DIGIT_6: [&str; 7] = glyph![
    " ### ", "#   #", "#    ", "#### ", "#   #", "#   #", " ### ",
];

const DIGIT_7: [&str; 7] = glyph![
    "#####", "    #", "   # ", "  #  ", " #   ", " #   ", " #   ",
];

const DIGIT_8: [&str; 7] = glyph![
    " ### ", "#   #", "#   #", " ### ", "#   #", "#   #", " ### ",
];

const DIGIT_9: [&str; 7] = glyph![
    " ### ", "#   #", "#   #", " ####", "    #", "#   #", " ### ",
];

const LETTER_A: [&str; 7] = glyph![
    " ### ", "#   #", "#   #", "#####", "#   #", "#   #", "#   #",
];

const LETTER_B: [&str; 7] = glyph![
    "#### ", "#   #", "#   #", "#### ", "#   #", "#   #", "#### ",
];

const LETTER_C: [&str; 7] = glyph![
    " ### ", "#   #", "#    ", "#    ", "#    ", "#   #", " ### ",
];

const LETTER_D: [&str; 7] = glyph![
    "#### ", "#   #", "#   #", "#   #", "#   #", "#   #", "#### ",
];

const LETTER_E: [&str; 7] = glyph![
    "#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#####",
];

const LETTER_F: [&str; 7] = glyph![
    "#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#    ",
];

const LETTER_G: [&str; 7] = glyph![
    " ### ", "#   #", "#    ", "# ###", "#   #", "#   #", " ### ",
];

const LETTER_H: [&str; 7] = glyph![
    "#   #", "#   #", "#   #", "#####", "#   #", "#   #", "#   #",
];

const LETTER_I: [&str; 7] = glyph![
    "#####", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "#####",
];

const LETTER_L: [&str; 7] = glyph![
    "#    ", "#    ", "#    ", "#    ", "#    ", "#    ", "#####",
];

const LETTER_M: [&str; 7] = glyph![
    "#   #", "## ##", "# # #", "#   #", "#   #", "#   #", "#   #",
];

const LETTER_N: [&str; 7] = glyph![
    "#   #", "##  #", "# # #", "#  ##", "#   #", "#   #", "#   #",
];

const LETTER_O: [&str; 7] = glyph![
    " ### ", "#   #", "#   #", "#   #", "#   #", "#   #", " ### ",
];

const LETTER_P: [&str; 7] = glyph![
    "#### ", "#   #", "#   #", "#### ", "#    ", "#    ", "#    ",
];

const LETTER_R: [&str; 7] = glyph![
    "#### ", "#   #", "#   #", "#### ", "# #  ", "#  # ", "#   #",
];

const LETTER_S: [&str; 7] = glyph![
    " ####", "#    ", "#    ", " ### ", "    #", "    #", "#### ",
];

const LETTER_T: [&str; 7] = glyph![
    "#####", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ",
];

const LETTER_U: [&str; 7] = glyph![
    "#   #", "#   #", "#   #", "#   #", "#   #", "#   #", " ### ",
];

const LETTER_V: [&str; 7] = glyph![
    "#   #", "#   #", "#   #", "#   #", "#   #", " # # ", "  #  ",
];

const GLYPH_COLON: [&str; 7] = glyph![
    "     ", "  #  ", "  #  ", "     ", "  #  ", "  #  ", "     ",
];

const GLYPH_PERCENT: [&str; 7] = glyph![
    "#   #", "    #", "   # ", "  #  ", " #   ", "#    ", "#   #",
];

const GLYPH_DASH: [&str; 7] = glyph![
    "     ", "     ", "     ", " ### ", "     ", "     ", "     ",
];
