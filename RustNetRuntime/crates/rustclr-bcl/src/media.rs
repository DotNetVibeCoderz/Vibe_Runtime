//! `RustNet.Media` — the image sensor.
//!
//! Four bindings over [`rustclr_hal::camera::Camera`], following the same rule
//! as `RustNet.Hal`: reach the board, turn a `HalError` into a managed
//! exception, and refuse rather than invent.
//!
//! # A board without a sensor throws, and does not return a blank frame
//!
//! Returning `new byte[w * h * 2]` from `Capture` on a board with no camera
//! would be the worst possible answer: every pixel black is a *plausible*
//! frame — a covered lens looks exactly like that — so a bring-up check would
//! pass on hardware that has no sensor attached at all. The default
//! `Board::camera` returns `NotSupported` and this turns it into an exception
//! naming the board, which is a thing a person can act on.
//!
//! # `Configure` before `Capture`, and the sizes must agree
//!
//! `Capture` allocates from the sensor's *configured* geometry, read back from
//! the driver rather than remembered here. A driver is allowed to round a
//! request to something the sensor supports — the OV2640 does — so a program
//! that asked for 320x240 and calls `Width()` gets what it actually got.

use crate::support::*;
use rustclr_core::{ExecResult, ExecutionError, Interpreter, Value};
use rustclr_hal::camera::{CameraConfig, PixelFormat};
use rustclr_hal::{Board, HalError};

#[allow(unused_imports)]
use crate::prelude::*;

fn no_board(what: &str) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::InvalidOperation,
        &format!(
            "{what} needs a board with a camera, and none is installed. \
             Run with `--board host` for the colour-bar test sensor, or flash a \
             firmware whose board has an image sensor."
        ),
    )
}

fn camera_error(what: &str, error: HalError) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::InvalidOperation,
        &format!("{what}: {error:?}"),
    )
}

/// Runs `f` against the board's sensor, or throws.
fn with_camera<T>(
    interp: &mut Interpreter,
    what: &str,
    f: impl FnOnce(&mut dyn rustclr_hal::camera::Camera) -> Result<T, HalError>,
) -> ExecResult<T> {
    let Some(board) = interp.board.as_mut() else {
        return Err(no_board(what));
    };
    let board: &mut dyn Board = board.as_mut();
    match board.camera() {
        Ok(camera) => f(camera).map_err(|e| camera_error(what, e)),
        // `NotSupported` is the default `Board::camera`, i.e. a board with no
        // sensor. Said in those words rather than as a bare `NotSupported`,
        // because that is the case a person actually hits.
        Err(HalError::NotSupported) => Err(no_board(what)),
        Err(e) => Err(camera_error(what, e)),
    }
}

/// Values match `RustNet.Media.PixelFormat` on the managed side.
fn pixel_format(value: i32) -> Result<PixelFormat, HalError> {
    Ok(match value {
        0 => PixelFormat::Rgb565,
        1 => PixelFormat::Grayscale,
        // Not defaulted to RGB565: a program that passed 4 meant something,
        // and a frame in the wrong format is bytes that decode into noise.
        _ => return Err(HalError::InvalidArgument("pixel format must be 0 or 1")),
    })
}

pub fn register(interp: &mut Interpreter) {
    interp.register_native(
        "RustNet.Media.Camera::ConfigureRaw(int,int,int)",
        |i, a| {
            let width = arg_i32(i, a, 0)?;
            let height = arg_i32(i, a, 1)?;
            let format = arg_i32(i, a, 2)?;
            with_camera(i, "Camera.Configure", |camera| {
                let format = pixel_format(format)?;
                // Negative dimensions are rejected here rather than cast: `as
                // u32` would turn -1 into four billion and the driver would
                // refuse it with a size nobody asked for in the message.
                if width <= 0 || height <= 0 {
                    return Err(HalError::InvalidArgument(
                        "camera dimensions must be positive",
                    ));
                }
                camera.configure(CameraConfig {
                    width: width as u32,
                    height: height as u32,
                    format,
                })
            })?;
            Ok(None)
        },
    );

    interp.register_native("RustNet.Media.Camera::Capture()", |i, _a| {
        let frame = with_camera(i, "Camera.Capture", |camera| camera.capture())?;
        Ok(Some(Value::Obj(byte_array(i, &frame))))
    });

    interp.register_native("RustNet.Media.Camera::Width()", |i, _a| {
        let width = with_camera(i, "Camera.Width", |camera| Ok(camera.config().width))?;
        Ok(Some(Value::I32(width as i32)))
    });

    interp.register_native("RustNet.Media.Camera::Height()", |i, _a| {
        let height = with_camera(i, "Camera.Height", |camera| Ok(camera.config().height))?;
        Ok(Some(Value::I32(height as i32)))
    });
}
