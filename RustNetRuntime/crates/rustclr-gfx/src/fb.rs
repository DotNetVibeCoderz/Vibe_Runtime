use alloc::vec;
use alloc::vec::Vec;
use crate::font::glyph;

/// RGB565 color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u16);

impl Color {
    pub const BLACK: Color = Color(0x0000);
    pub const WHITE: Color = Color(0xFFFF);
    pub const RED: Color = Color(0xF800);
    pub const GREEN: Color = Color(0x07E0);
    pub const BLUE: Color = Color(0x001F);
    pub const YELLOW: Color = Color(0xFFE0);
    pub const CYAN: Color = Color(0x07FF);
    pub const MAGENTA: Color = Color(0xF81F);

    pub fn from_rgb(r: u8, g: u8, b: u8) -> Color {
        Color(((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3))
    }

    pub fn to_rgb(self) -> (u8, u8, u8) {
        let r = ((self.0 >> 11) & 0x1F) as u8;
        let g = ((self.0 >> 5) & 0x3F) as u8;
        let b = (self.0 & 0x1F) as u8;
        (r << 3, g << 2, b << 3)
    }

    /// Perceived brightness 0-255 (for mono displays).
    pub fn luma(self) -> u8 {
        let (r, g, b) = self.to_rgb();
        ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8
    }
}

/// In-memory RGB565 surface with immediate-mode drawing primitives.
///
/// `width`/`height`/`pixels` are the *physical* panel buffer. Drawing happens
/// in a *logical* coordinate space that a panel `rotation` (0/90/180/270°
/// clockwise) maps onto the physical buffer, optionally masked to a `clip`
/// rectangle — so a panel mounted sideways or upside-down, and scrolled
/// containers that must not overdraw, both work without the app knowing.
#[derive(Clone)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u16>,
    /// Clockwise panel rotation in degrees: 0, 90, 180 or 270.
    rotation: u16,
    /// Active clip rectangle in logical coords (x, y, w, h); None = no clip.
    clip: Option<(i32, i32, i32, i32)>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height) as usize],
            rotation: 0,
            clip: None,
        }
    }

    /// Set the clockwise panel rotation (degrees, snapped to 0/90/180/270).
    pub fn set_rotation(&mut self, degrees: u16) {
        self.rotation = match ((degrees as u32 + 45) / 90 * 90) % 360 {
            90 => 90,
            180 => 180,
            270 => 270,
            _ => 0,
        };
    }

    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    /// Logical drawing size, accounting for rotation (90/270 swap w/h).
    pub fn logical_size(&self) -> (u32, u32) {
        if self.rotation == 90 || self.rotation == 270 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// Constrain subsequent drawing to a logical rectangle.
    pub fn set_clip(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.clip = Some((x, y, w.max(0), h.max(0)));
    }

    pub fn clear_clip(&mut self) {
        self.clip = None;
    }

    /// Map a logical coordinate to a physical buffer index, honouring the
    /// clip rectangle and rotation. Returns None if out of bounds/clipped.
    #[inline]
    fn phys_index(&self, x: i32, y: i32, apply_clip: bool) -> Option<usize> {
        let (lw, lh) = self.logical_size();
        if x < 0 || y < 0 || (x as u32) >= lw || (y as u32) >= lh {
            return None;
        }
        if apply_clip {
            if let Some((cx, cy, cw, ch)) = self.clip {
                if x < cx || y < cy || x >= cx + cw || y >= cy + ch {
                    return None;
                }
            }
        }
        let (px, py) = match self.rotation {
            90 => (self.width as i32 - 1 - y, x),
            180 => (self.width as i32 - 1 - x, self.height as i32 - 1 - y),
            270 => (y, self.height as i32 - 1 - x),
            _ => (x, y),
        };
        Some((py as u32 * self.width + px as u32) as usize)
    }

    #[inline]
    pub fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        // Drawing respects the clip rectangle.
        if let Some(i) = self.phys_index(x, y, true) {
            self.pixels[i] = color.0;
        }
    }

    /// Read a logical pixel. Reads ignore the clip rectangle (clip only
    /// masks drawing).
    pub fn get_pixel(&self, x: i32, y: i32) -> Option<Color> {
        self.phys_index(x, y, false).map(|i| Color(self.pixels[i]))
    }

    pub fn clear(&mut self, color: Color) {
        self.pixels.fill(color.0);
    }

    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        // Bresenham
        let (mut x, mut y) = (x0, y0);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.set_pixel(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    pub fn draw_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.draw_line(x, y, x + w - 1, y, color);
        self.draw_line(x, y + h - 1, x + w - 1, y + h - 1, color);
        self.draw_line(x, y, x, y + h - 1, color);
        self.draw_line(x + w - 1, y, x + w - 1, y + h - 1, color);
    }

    /// Blit an RGB565 pixel buffer (`w*h`, row-major) at (x, y). Honours the
    /// active clip rectangle and panel rotation. One call places a whole
    /// decoded image.
    pub fn draw_image(&mut self, x: i32, y: i32, w: u32, h: u32, src: &[u16]) {
        for row in 0..h {
            for col in 0..w {
                if let Some(px) = src.get((row * w + col) as usize) {
                    self.set_pixel(x + col as i32, y + row as i32, Color(*px));
                }
            }
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set_pixel(xx, yy, color);
            }
        }
    }

    /// Fill a rectangle with a linear gradient interpolating `c0` -> `c1`
    /// in RGB space. `vertical` runs top->bottom, otherwise left->right.
    pub fn fill_gradient(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        c0: Color,
        c1: Color,
        vertical: bool,
    ) {
        if w <= 0 || h <= 0 {
            return;
        }
        let (r0, g0, b0) = c0.to_rgb();
        let (r1, g1, b1) = c1.to_rgb();
        let span = if vertical { h } else { w };
        let denom = (span - 1).max(1);
        for yy in 0..h {
            for xx in 0..w {
                let t = if vertical { yy } else { xx };
                let lerp = |a: u8, b: u8| -> u8 {
                    (a as i32 + (b as i32 - a as i32) * t / denom) as u8
                };
                let c = Color::from_rgb(lerp(r0, r1), lerp(g0, g1), lerp(b0, b1));
                self.set_pixel(x + xx, y + yy, c);
            }
        }
    }

    /// Alpha-blend `src` (color) over the pixel at (x, y). `alpha` is 0
    /// (transparent, keep background) to 255 (opaque, replace).
    #[inline]
    pub fn blend_pixel(&mut self, x: i32, y: i32, src: Color, alpha: u8) {
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.set_pixel(x, y, src);
            return;
        }
        if let Some(dst) = self.get_pixel(x, y) {
            let (sr, sg, sb) = src.to_rgb();
            let (dr, dg, db) = dst.to_rgb();
            let a = alpha as u32;
            let ia = 255 - a;
            let mix = |s: u8, d: u8| -> u8 {
                ((s as u32 * a + d as u32 * ia) / 255) as u8
            };
            self.set_pixel(x, y, Color::from_rgb(mix(sr, dr), mix(sg, dg), mix(sb, db)));
        }
    }

    /// Blit an RGB565 image over the framebuffer with a global `alpha`
    /// (0-255), blending each source pixel with the background.
    /// Blit `src` with a per-pixel alpha channel.
    ///
    /// The alpha travels beside the colour rather than inside it because the
    /// framebuffer is RGB565: there is no spare bit for coverage, and widening
    /// every pixel to carry one would double a 150 KB frame to buy something
    /// only some images use. A parallel `u8` per pixel costs 50% more for the
    /// images that need it and nothing at all for the ones that do not.
    ///
    /// A short `alpha` is treated as opaque from where it runs out. That is
    /// the forgiving direction: an image that loses its mask should appear,
    /// not vanish.
    pub fn draw_image_alpha(&mut self, x: i32, y: i32, w: u32, h: u32, src: &[u16], alpha: &[u8]) {
        for row in 0..h {
            for col in 0..w {
                let i = (row * w + col) as usize;
                if let Some(px) = src.get(i) {
                    let a = alpha.get(i).copied().unwrap_or(255);
                    self.blend_pixel(x + col as i32, y + row as i32, Color(*px), a);
                }
            }
        }
    }

    pub fn blend_image(&mut self, x: i32, y: i32, w: u32, h: u32, src: &[u16], alpha: u8) {
        for row in 0..h {
            for col in 0..w {
                if let Some(px) = src.get((row * w + col) as usize) {
                    self.blend_pixel(x + col as i32, y + row as i32, Color(*px), alpha);
                }
            }
        }
    }

    pub fn draw_circle(&mut self, cx: i32, cy: i32, r: i32, color: Color) {
        // Midpoint circle
        let (mut x, mut y, mut d) = (r, 0, 1 - r);
        while x >= y {
            for (px, py) in [
                (cx + x, cy + y), (cx - x, cy + y), (cx + x, cy - y), (cx - x, cy - y),
                (cx + y, cy + x), (cx - y, cy + x), (cx + y, cy - x), (cx - y, cy - x),
            ] {
                self.set_pixel(px, py, color);
            }
            y += 1;
            if d < 0 {
                d += 2 * y + 1;
            } else {
                x -= 1;
                d += 2 * (y - x) + 1;
            }
        }
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, color: Color) {
        for yy in -r..=r {
            for xx in -r..=r {
                if xx * xx + yy * yy <= r * r {
                    self.set_pixel(cx + xx, cy + yy, color);
                }
            }
        }
    }

    /// Draw text with the built-in 8x8 font. `scale` >= 1.
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: Color, scale: i32) {
        let scale = scale.max(1);
        let mut cursor_x = x;
        for ch in text.chars() {
            let glyph = glyph(ch);
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (1 << col) != 0 {
                        let px = cursor_x + col as i32 * scale;
                        let py = y + row as i32 * scale;
                        self.fill_rect(px, py, scale, scale, color);
                    }
                }
            }
            cursor_x += 8 * scale;
        }
    }

    /// Pixel width of a string at the given scale.
    pub fn text_width(text: &str, scale: i32) -> i32 {
        text.chars().count() as i32 * 8 * scale.max(1)
    }

    /// Copy another framebuffer onto this one at (x, y).
    pub fn blit(&mut self, src: &Framebuffer, x: i32, y: i32) {
        for sy in 0..src.height as i32 {
            for sx in 0..src.width as i32 {
                if let Some(c) = src.get_pixel(sx, sy) {
                    self.set_pixel(x + sx, y + sy, c);
                }
            }
        }
    }

    /// Load a raw RGB565 big-endian byte buffer as a bitmap.
    pub fn from_rgb565_bytes(width: u32, height: u32, data: &[u8]) -> Option<Framebuffer> {
        if data.len() != (width * height * 2) as usize {
            return None;
        }
        let pixels = data
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        Some(Framebuffer { width, height, pixels, rotation: 0, clip: None })
    }
}
