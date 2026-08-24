use alloc::boxed::Box;

use crate::fb::Framebuffer;
use rustclr_hal::HalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A physical display: reports its size and accepts framebuffer regions.
pub trait DisplayDriver: Send {
    fn size(&self) -> (u32, u32);
    fn flush(&mut self, fb: &Framebuffer, region: Rect) -> Result<(), HalError>;
}

/// Double-buffered display frontend. Draw on `canvas()`, then `present()`
/// diffs against the front buffer and flushes only the dirty rectangle —
/// the trick that makes animation smooth on slow SPI/I2C panels.
pub struct Display {
    driver: Box<dyn DisplayDriver>,
    back: Framebuffer,
    front: Framebuffer,
    first_present: bool,
}

impl Display {
    pub fn new(driver: Box<dyn DisplayDriver>) -> Self {
        let (w, h) = driver.size();
        Self {
            driver,
            back: Framebuffer::new(w, h),
            front: Framebuffer::new(w, h),
            first_present: true,
        }
    }

    pub fn canvas(&mut self) -> &mut Framebuffer {
        &mut self.back
    }

    pub fn size(&self) -> (u32, u32) {
        (self.back.width, self.back.height)
    }

    /// Flush changes to the panel. Returns the flushed region, if any.
    pub fn present(&mut self) -> Result<Option<Rect>, HalError> {
        let region = if self.first_present {
            self.first_present = false;
            Some(Rect { x: 0, y: 0, w: self.back.width, h: self.back.height })
        } else {
            self.dirty_rect()
        };
        if let Some(r) = region {
            self.driver.flush(&self.back, r)?;
            self.front.pixels.copy_from_slice(&self.back.pixels);
        }
        Ok(region)
    }

    fn dirty_rect(&self) -> Option<Rect> {
        let (w, h) = (self.back.width, self.back.height);
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0u32, 0u32);
        let mut dirty = false;
        for y in 0..h {
            let row = (y * w) as usize;
            let back_row = &self.back.pixels[row..row + w as usize];
            let front_row = &self.front.pixels[row..row + w as usize];
            if back_row == front_row {
                continue;
            }
            for x in 0..w as usize {
                if back_row[x] != front_row[x] {
                    dirty = true;
                    min_x = min_x.min(x as u32);
                    max_x = max_x.max(x as u32);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        dirty.then(|| Rect { x: min_x, y: min_y, w: max_x - min_x + 1, h: max_y - min_y + 1 })
    }
}
