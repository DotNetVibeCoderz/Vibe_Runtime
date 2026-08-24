//! Graphics library: System.Drawing-like immediate-mode API over an
//! RGB565 framebuffer, 8x8 font text rendering, double buffering with
//! dirty-rectangle flush, and display drivers (SSD1306 OLED over I2C,
//! ST7735 TFT over SPI) built on the RustNet HAL.
//!
//! Builds `no_std + alloc` when the default `std` feature is disabled, which
//! is what lets a bare-metal port put a real panel behind the same
//! `Display`/`Framebuffer` the virtual device draws into.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod fb;
mod display;
pub mod font;
pub mod drivers;

pub use display::{Display, DisplayDriver, Rect};
pub use fb::{Color, Framebuffer};

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn pixels_lines_rects() {
        let mut fb = Framebuffer::new(32, 32);
        fb.clear(Color::BLACK);
        fb.set_pixel(1, 1, Color::WHITE);
        assert_eq!(fb.get_pixel(1, 1), Some(Color::WHITE));
        assert_eq!(fb.get_pixel(2, 1), Some(Color::BLACK));
        // Out of bounds is a no-op, not a panic.
        fb.set_pixel(100, 100, Color::WHITE);
        assert_eq!(fb.get_pixel(100, 100), None);

        fb.draw_line(0, 0, 31, 31, Color::RED);
        assert_eq!(fb.get_pixel(15, 15), Some(Color::RED));

        fb.fill_rect(4, 4, 8, 8, Color::GREEN);
        assert_eq!(fb.get_pixel(4, 4), Some(Color::GREEN));
        assert_eq!(fb.get_pixel(11, 11), Some(Color::GREEN));
        assert_eq!(fb.get_pixel(12, 12), Some(Color::RED)); // diagonal continues

        fb.draw_rect(0, 0, 32, 32, Color::BLUE);
        assert_eq!(fb.get_pixel(0, 31), Some(Color::BLUE));
        assert_eq!(fb.get_pixel(31, 0), Some(Color::BLUE));
    }

    #[test]
    fn gradient_and_alpha_blend() {
        // Horizontal gradient black -> white across 16 px: left dark, right bright.
        let mut fb = Framebuffer::new(16, 4);
        fb.fill_gradient(0, 0, 16, 4, Color::BLACK, Color::WHITE, false);
        let left = fb.get_pixel(0, 0).unwrap().luma();
        let right = fb.get_pixel(15, 0).unwrap().luma();
        assert_eq!(left, 0, "gradient start should be black");
        assert!(right > 240, "gradient end should be near-white, got {right}");
        assert!(fb.get_pixel(8, 0).unwrap().luma() > left, "midpoint brighter than start");

        // Alpha blend: 50% white over black -> mid grey (~127 per channel).
        let mut fb2 = Framebuffer::new(4, 4);
        fb2.clear(Color::BLACK);
        fb2.blend_pixel(0, 0, Color::WHITE, 128);
        let (r, g, b) = fb2.get_pixel(0, 0).unwrap().to_rgb();
        assert!((120..=136).contains(&r) && (120..=136).contains(&g) && (120..=136).contains(&b),
            "50% blend should be ~mid grey, got {r},{g},{b}");
        // alpha 0 keeps background, 255 replaces it.
        fb2.blend_pixel(1, 1, Color::WHITE, 0);
        assert_eq!(fb2.get_pixel(1, 1), Some(Color::BLACK));
        fb2.blend_pixel(2, 2, Color::WHITE, 255);
        assert_eq!(fb2.get_pixel(2, 2), Some(Color::WHITE));
    }

    #[test]
    fn clip_and_rotation() {
        // Clip: only pixels inside the clip rect are drawn.
        let mut fb = Framebuffer::new(16, 16);
        fb.clear(Color::BLACK);
        fb.set_clip(4, 4, 4, 4);
        fb.fill_rect(0, 0, 16, 16, Color::WHITE);
        assert_eq!(fb.get_pixel(0, 0), Some(Color::BLACK), "outside clip untouched");
        assert_eq!(fb.get_pixel(5, 5), Some(Color::WHITE), "inside clip drawn");
        assert_eq!(fb.get_pixel(8, 8), Some(Color::BLACK), "past clip edge untouched");
        fb.clear_clip();
        fb.set_pixel(0, 0, Color::WHITE);
        assert_eq!(fb.get_pixel(0, 0), Some(Color::WHITE), "clip cleared");

        // Rotation 90: logical size swaps; logical (0,0) lands top-right.
        let mut r = Framebuffer::new(8, 4); // physical 8x4
        r.set_rotation(90);
        assert_eq!(r.logical_size(), (4, 8)); // logical 4x8
        r.clear(Color::BLACK);
        r.set_pixel(0, 0, Color::RED);
        assert_eq!(r.pixels[7], Color::RED.0, "logical (0,0) -> physical (7,0)");
        assert_eq!(r.get_pixel(0, 0), Some(Color::RED), "logical read-back consistent");

        // 180 keeps dims, flips both axes.
        let mut r2 = Framebuffer::new(4, 4);
        r2.set_rotation(180);
        r2.clear(Color::BLACK);
        r2.set_pixel(0, 0, Color::GREEN);
        assert_eq!(r2.pixels[15], Color::GREEN.0, "physical bottom-right");
        assert_eq!(r2.get_pixel(0, 0), Some(Color::GREEN));
        // Snapping: 89 -> 90, 200 -> 180.
        r2.set_rotation(89);
        assert_eq!(r2.rotation(), 90);
    }

    #[test]
    fn circle_and_text() {
        let mut fb = Framebuffer::new(64, 64);
        fb.clear(Color::BLACK);
        fb.fill_circle(32, 32, 10, Color::WHITE);
        assert_eq!(fb.get_pixel(32, 32), Some(Color::WHITE));
        assert_eq!(fb.get_pixel(32, 41), Some(Color::WHITE));
        assert_eq!(fb.get_pixel(32, 44), Some(Color::BLACK));

        let mut fb2 = Framebuffer::new(64, 16);
        fb2.clear(Color::BLACK);
        fb2.draw_text(0, 0, "Hi!", Color::WHITE, 1);
        // Some pixels must be lit and the glyph cell for 'H' has its
        // left column set.
        let lit = (0..64)
            .flat_map(|x| (0..16).map(move |y| (x, y)))
            .filter(|&(x, y)| fb2.get_pixel(x, y) == Some(Color::WHITE))
            .count();
        assert!(lit > 10, "text rendered only {lit} pixels");
    }

    #[test]
    fn bitmap_blit_and_scaling() {
        let mut src = Framebuffer::new(4, 4);
        src.clear(Color::RED);
        let mut dst = Framebuffer::new(16, 16);
        dst.clear(Color::BLACK);
        dst.blit(&src, 6, 6);
        assert_eq!(dst.get_pixel(6, 6), Some(Color::RED));
        assert_eq!(dst.get_pixel(9, 9), Some(Color::RED));
        assert_eq!(dst.get_pixel(10, 10), Some(Color::BLACK));

        let mut dst2 = Framebuffer::new(16, 16);
        dst2.clear(Color::BLACK);
        dst2.draw_text(0, 0, "A", Color::WHITE, 2); // 2x scale = 16x16 glyph
        let lit = (0..16)
            .flat_map(|x| (0..16).map(move |y| (x, y)))
            .filter(|&(x, y)| dst2.get_pixel(x, y) == Some(Color::WHITE))
            .count();
        assert!(lit > 40, "scaled glyph too sparse: {lit}");
    }

    #[test]
    fn double_buffer_flushes_only_dirty_region() {
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct RecordingDriver {
            flushes: Arc<Mutex<Vec<Rect>>>,
        }
        impl DisplayDriver for RecordingDriver {
            fn size(&self) -> (u32, u32) {
                (32, 32)
            }
            fn flush(
                &mut self,
                _fb: &Framebuffer,
                region: Rect,
            ) -> Result<(), rustclr_hal::HalError> {
                self.flushes.lock().unwrap().push(region);
                Ok(())
            }
        }

        let flushes = Arc::new(Mutex::new(Vec::new()));
        let driver = RecordingDriver { flushes: flushes.clone() };
        let mut display = Display::new(Box::new(driver));
        display.canvas().clear(Color::BLACK);
        display.present().unwrap();
        // Full-screen first flush.
        assert_eq!(flushes.lock().unwrap()[0], Rect { x: 0, y: 0, w: 32, h: 32 });

        // Touch a small area: only that region flushes.
        display.canvas().fill_rect(8, 8, 4, 4, Color::RED);
        display.present().unwrap();
        let last = *flushes.lock().unwrap().last().unwrap();
        assert_eq!(last, Rect { x: 8, y: 8, w: 4, h: 4 });

        // No change → no flush.
        let count = flushes.lock().unwrap().len();
        display.present().unwrap();
        assert_eq!(flushes.lock().unwrap().len(), count);
    }

    /// A mask makes some pixels land and others not, and the ones in between
    /// mix. Without the middle case this would pass for a driver that treated
    /// alpha as a one-bit stencil.
    #[test]
    fn a_mask_decides_which_pixels_land() {
        let mut fb = Framebuffer::new(3, 1);
        fb.fill_rect(0, 0, 3, 1, Color::BLACK);
        let src = [Color::WHITE.0; 3];
        fb.draw_image_alpha(0, 0, 3, 1, &src, &[255, 0, 128]);

        assert_eq!(fb.get_pixel(0, 0), Some(Color::WHITE), "opaque pixel");
        assert_eq!(fb.get_pixel(1, 0), Some(Color::BLACK), "transparent pixel");
        let mid = fb.get_pixel(2, 0).unwrap();
        assert_ne!(mid, Color::WHITE);
        assert_ne!(mid, Color::BLACK);
    }

    /// A mask shorter than the image leaves the rest opaque. The forgiving
    /// direction: an image that loses its mask should appear, not vanish.
    #[test]
    fn a_short_mask_leaves_the_rest_opaque() {
        let mut fb = Framebuffer::new(2, 1);
        fb.fill_rect(0, 0, 2, 1, Color::BLACK);
        let src = [Color::WHITE.0; 2];
        fb.draw_image_alpha(0, 0, 2, 1, &src, &[0]);

        assert_eq!(fb.get_pixel(0, 0), Some(Color::BLACK));
        assert_eq!(fb.get_pixel(1, 0), Some(Color::WHITE));
    }
}
