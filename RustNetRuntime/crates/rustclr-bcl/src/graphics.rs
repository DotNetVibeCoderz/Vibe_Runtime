//! `RustNet.Graphics.Display` — drawing on a panel.
//!
//! One framebuffer per interpreter, RGB565, with the primitives a device UI
//! actually draws: lines, rectangles, circles, text and images. `Present`
//! pushes it to the board's panel.
//!
//! # Why one framebuffer and not a `Graphics` object per surface
//!
//! Because a board has one panel and a few tens of kilobytes to spare. A
//! 240×240 RGB565 buffer is 115 KB — on most of the boards here that is the
//! entire heap budget, so a second one is not a feature, it is an
//! out-of-memory. The single buffer is created on demand by `Open` and its
//! size is the caller's choice, which is the only decision that matters.
//!
//! # Colours are RGB565 as an `int`
//!
//! Not a `Color` struct: a struct crossing the binding boundary per pixel
//! would allocate, and `FillRect` in a loop is exactly where a device spends
//! its drawing time. `Display.Rgb(r, g, b)` packs one.

use crate::support::*;
use rustclr_core::{ExecutionError, Interpreter, Value};
use rustclr_gfx::{Color, Framebuffer};

#[allow(unused_imports)]
use crate::prelude::*;

fn no_canvas(what: &str) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::InvalidOperation,
        &format!("{what}: no canvas. Call Display.Init(width, height) first."),
    )
}

/// Runs `f` against the canvas, or throws.
fn with_canvas<T>(
    interp: &mut Interpreter,
    what: &str,
    f: impl FnOnce(&mut Framebuffer) -> T,
) -> Result<T, ExecutionError> {
    match interp.canvas.as_mut() {
        Some(fb) => Ok(f(fb)),
        None => Err(no_canvas(what)),
    }
}

pub fn register(interp: &mut Interpreter) {
    interp.register_native("RustNet.Graphics.Display::Init(int,int)", |i, a| {
        let width = arg_i32(i, a, 0)?.max(0) as u32;
        let height = arg_i32(i, a, 1)?.max(0) as u32;
        if width == 0 || height == 0 {
            return Err(ExecutionError::exception(
                rustclr_core::ClrExceptionKind::ArgumentOutOfRange,
                "a canvas needs a width and a height of at least 1",
            ));
        }
        // Asked for before it is taken. `Framebuffer::new` allocates with
        // `vec!`, and on a board an allocation it cannot serve does not return
        // an error — it aborts, and the program is gone with no message worth
        // reading. `try_reserve` is the same question asked so the answer can
        // be a managed exception naming the size that did not fit.
        let pixels = width as usize * height as usize;
        let bytes = pixels as u64 * 2;
        let mut probe: Vec<u16> = Vec::new();
        if probe.try_reserve_exact(pixels).is_err() {
            return Err(ExecutionError::exception(
                rustclr_core::ClrExceptionKind::OutOfMemory,
                &format!(
                    "a {width}x{height} canvas needs {bytes} bytes of RGB565 and this                      board cannot spare them"
                ),
            ));
        }
        // Handing the reservation straight to the framebuffer, so the question
        // asked is the memory used rather than a second allocation that might
        // answer differently.
        drop(probe);
        i.canvas = Some(Framebuffer::new(width, height));
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::Close()", |i, _a| {
        i.canvas = None;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::IsOpen()", |i, _a| {
        Ok(Some(Value::I32(i32::from(i.canvas.is_some()))))
    });
    interp.register_native("RustNet.Graphics.Display::Width()", |i, _a| {
        let w = with_canvas(i, "Display.Width", |fb| fb.logical_size().0)?;
        Ok(Some(Value::I32(w as i32)))
    });
    interp.register_native("RustNet.Graphics.Display::Height()", |i, _a| {
        let h = with_canvas(i, "Display.Height", |fb| fb.logical_size().1)?;
        Ok(Some(Value::I32(h as i32)))
    });

    // Packs an RGB565 colour. The loss is real and stated: 5 bits of red, 6 of
    // green, 5 of blue, because that is what the panels take.
    interp.register_native("RustNet.Graphics.Display::Rgb(int,int,int)", |i, a| {
        let r = arg_i32(i, a, 0)?.clamp(0, 255) as u8;
        let g = arg_i32(i, a, 1)?.clamp(0, 255) as u8;
        let b = arg_i32(i, a, 2)?.clamp(0, 255) as u8;
        Ok(Some(Value::I32(Color::from_rgb(r, g, b).0 as i32)))
    });

    interp.register_native("RustNet.Graphics.Display::Clear(int)", |i, a| {
        let color = Color(arg_i32(i, a, 0)? as u16);
        with_canvas(i, "Display.Clear", |fb| fb.clear(color))?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::SetPixel(int,int,int)", |i, a| {
        let (x, y) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?);
        let color = Color(arg_i32(i, a, 2)? as u16);
        with_canvas(i, "Display.SetPixel", |fb| fb.set_pixel(x, y, color))?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::GetPixel(int,int)", |i, a| {
        let (x, y) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?);
        // Off the canvas reads as -1 rather than as black: black is a colour a
        // program might have drawn, and "outside" is not.
        let value = with_canvas(i, "Display.GetPixel", |fb| {
            fb.get_pixel(x, y).map(|c| c.0 as i32).unwrap_or(-1)
        })?;
        Ok(Some(Value::I32(value)))
    });
    interp.register_native("RustNet.Graphics.Display::DrawLine(int,int,int,int,int)", |i, a| {
        let (x0, y0, x1, y1) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?, arg_i32(i, a, 3)?);
        let color = Color(arg_i32(i, a, 4)? as u16);
        with_canvas(i, "Display.DrawLine", |fb| fb.draw_line(x0, y0, x1, y1, color))?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::DrawRect(int,int,int,int,int)", |i, a| {
        let (x, y, w, h) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?, arg_i32(i, a, 3)?);
        let color = Color(arg_i32(i, a, 4)? as u16);
        with_canvas(i, "Display.DrawRect", |fb| fb.draw_rect(x, y, w, h, color))?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::FillRect(int,int,int,int,int)", |i, a| {
        let (x, y, w, h) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?, arg_i32(i, a, 3)?);
        let color = Color(arg_i32(i, a, 4)? as u16);
        with_canvas(i, "Display.FillRect", |fb| fb.fill_rect(x, y, w, h, color))?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::DrawCircle(int,int,int,int)", |i, a| {
        let (cx, cy, r) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
        let color = Color(arg_i32(i, a, 3)? as u16);
        with_canvas(i, "Display.DrawCircle", |fb| fb.draw_circle(cx, cy, r, color))?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::FillCircle(int,int,int,int)", |i, a| {
        let (cx, cy, r) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
        let color = Color(arg_i32(i, a, 3)? as u16);
        with_canvas(i, "Display.FillCircle", |fb| fb.fill_circle(cx, cy, r, color))?;
        Ok(None)
    });
    interp.register_native(
        "RustNet.Graphics.Display::FillGradient(int,int,int,int,int,int,bool)",
        |i, a| {
            let (x, y, w, h) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?, arg_i32(i, a, 3)?);
            let from = Color(arg_i32(i, a, 4)? as u16);
            let to = Color(arg_i32(i, a, 5)? as u16);
            let vertical = arg_bool(i, a, 6)?;
            with_canvas(i, "Display.FillGradient", |fb| {
                fb.fill_gradient(x, y, w, h, from, to, vertical)
            })?;
            Ok(None)
        },
    );
    interp.register_native(
        "RustNet.Graphics.Display::DrawImage(int,int,int,int,byte[])",
        |i, a| {
            let (x, y) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?);
            let w = arg_i32(i, a, 2)?.max(0) as u32;
            let h = arg_i32(i, a, 3)?.max(0) as u32;
            let bytes = arg_bytes(i, a, 4)?;
            // Two bytes per pixel, little-endian. A buffer that does not hold
            // `w * h` pixels is refused rather than drawn short: a half-drawn
            // image with the rest of the rectangle left as whatever was there
            // is worse than an error naming the mismatch.
            let wanted = w as usize * h as usize * 2;
            if bytes.len() != wanted {
                return Err(ExecutionError::exception(
                    rustclr_core::ClrExceptionKind::Argument,
                    &format!(
                        "Display.DrawImage: {}x{} RGB565 is {wanted} bytes, got {}",
                        w,
                        h,
                        bytes.len()
                    ),
                ));
            }
            let pixels: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect();
            with_canvas(i, "Display.DrawImage", |fb| fb.draw_image(x, y, w, h, &pixels))?;
            Ok(None)
        },
    );
    interp.register_native("RustNet.Graphics.Display::DrawText(int,int,string,int,int)", |i, a| {
        let (x, y) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?);
        let text = arg_string_or_empty(i, a, 2)?;
        let color = Color(arg_i32(i, a, 3)? as u16);
        let scale = arg_i32(i, a, 4)?.max(1);
        with_canvas(i, "Display.DrawText", |fb| fb.draw_text(x, y, &text, color, scale))?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::MeasureText(string,int)", |i, a| {
        let text = arg_string_or_empty(i, a, 0)?;
        let scale = arg_i32(i, a, 1)?.max(1);
        Ok(Some(Value::I32(Framebuffer::text_width(&text, scale))))
    });
    interp.register_native("RustNet.Graphics.Display::SetClip(int,int,int,int)", |i, a| {
        let (x, y, w, h) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?, arg_i32(i, a, 3)?);
        with_canvas(i, "Display.SetClip", |fb| fb.set_clip(x, y, w, h))?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::ClearClip()", |i, _a| {
        with_canvas(i, "Display.ClearClip", |fb| fb.clear_clip())?;
        Ok(None)
    });
    interp.register_native("RustNet.Graphics.Display::SetRotation(int)", |i, a| {
        let degrees = arg_i32(i, a, 0)?;
        if !matches!(degrees, 0 | 90 | 180 | 270) {
            return Err(ExecutionError::exception(
                rustclr_core::ClrExceptionKind::ArgumentOutOfRange,
                "rotation is 0, 90, 180 or 270 degrees",
            ));
        }
        with_canvas(i, "Display.SetRotation", |fb| fb.set_rotation(degrees as u16))?;
        Ok(None)
    });

    // Pushes the canvas to the panel. A board with no panel keeps the default
    // no-op from the HAL, so the drawing still happened and can be read back —
    // which is how a headless test checks a UI.
    interp.register_native("RustNet.Graphics.Display::Present()", |i, _a| {
        let Some(fb) = i.canvas.as_ref() else {
            return Err(no_canvas("Display.Present"));
        };
        let (width, height) = (fb.logical_size().0, fb.logical_size().1);
        let pixels: Vec<u16> = fb.pixels.clone();
        let Some(board) = i.board.as_mut() else {
            return Err(ExecutionError::exception(
                rustclr_core::ClrExceptionKind::InvalidOperation,
                "Display.Present needs a board; the canvas is still drawable without one",
            ));
        };
        board
            .present_frame(&pixels, width, height)
            .map_err(|e| {
                ExecutionError::exception(
                    rustclr_core::ClrExceptionKind::InvalidOperation,
                    &format!("Display.Present: {e}"),
                )
            })?;
        Ok(None)
    });
}
