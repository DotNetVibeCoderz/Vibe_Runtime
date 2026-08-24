//! `RustNet.Hal` — the managed face of the board underneath.
//!
//! Every binding here is the same three steps: read the arguments, reach the
//! board through [`Interpreter::board`], and turn a [`HalError`] into a managed
//! exception. The board itself is chosen at firmware build time, so
//! `RustNet.Hal.Gpio.Write(2, true)` drives a real pin on an ESP32, a simulated
//! one under `rustclr-hal-host`, and throws on a desktop with no board
//! installed.
//!
//! # No board is an error, not a no-op
//!
//! A program that writes a pin on a machine that has none is not doing
//! something harmless — it is doing something that will not happen. Rather than
//! return quietly, every binding throws `InvalidOperationException` naming what
//! was missing. That is the same rule the rest of the runtime follows: refuse
//! rather than guess.
//!
//! # Units are in the names
//!
//! `ReadMillivolts`, `dutyPermyriad`, `frequencyHz`, `micros`. An embedded API
//! whose numbers are unlabelled is one where a factor of a thousand is a
//! runtime surprise; putting the unit in the parameter name makes the mistake
//! visible at the call site.

use crate::support::*;
use rustclr_core::{ExecResult, ExecutionError, Interpreter, Value};
use rustclr_hal::gpio::{Level, PinMode};
use rustclr_hal::{Board, HalError};

#[allow(unused_imports)]
use crate::prelude::*;

/// Turns a HAL failure into the managed exception a C# program catches.
fn hal_error(what: &str, e: HalError) -> ExecutionError {
    let kind = match e {
        // A pin or peripheral this chip does not have is a program error about
        // the board it was written for, which is what `PlatformNotSupported`
        // means in .NET.
        HalError::NotSupported => rustclr_core::ClrExceptionKind::NotSupported,
        HalError::InvalidArgument(_) => rustclr_core::ClrExceptionKind::ArgumentOutOfRange,
        // No `TimeoutException` in the kind list, and inventing one here
        // would give it a type name nothing else in the runtime knows. The
        // message still says `timeout`, which is what a `catch` block reads.
        HalError::Timeout => rustclr_core::ClrExceptionKind::Io,
        _ => rustclr_core::ClrExceptionKind::InvalidOperation,
    };
    ExecutionError::exception(kind, &format!("{what}: {e}"))
}

/// The exception thrown when a program reaches for hardware that is not there.
fn no_board(what: &str) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::InvalidOperation,
        &format!(
            "{what} needs a board, and none is installed. \
             Run with `--board host` for the simulator, or flash a firmware that installs one."
        ),
    )
}

/// Runs `f` against the installed board, or throws.
///
/// Every binding in this file goes through here, so "no board" is phrased once
/// and a binding cannot forget to check.
fn with_board<T>(
    interp: &mut Interpreter,
    what: &str,
    f: impl FnOnce(&mut dyn Board) -> Result<T, HalError>,
) -> ExecResult<T> {
    let Some(board) = interp.board.as_mut() else {
        return Err(no_board(what));
    };
    f(board.as_mut()).map_err(|e| hal_error(what, e))
}

/// Values match `RustNet.Hal.PinMode` on the managed side.
fn pin_mode(value: i32) -> Result<PinMode, HalError> {
    Ok(match value {
        0 => PinMode::Input,
        1 => PinMode::InputPullUp,
        2 => PinMode::InputPullDown,
        3 => PinMode::Output,
        4 => PinMode::OutputOpenDrain,
        // Not clamped to the nearest mode: a program that passed 7 meant
        // something, and quietly making the pin an input would hide it.
        _ => return Err(HalError::InvalidArgument("pin mode must be 0..=4")),
    })
}

pub fn register(interp: &mut Interpreter) {
    register_gpio(interp);
    register_analog(interp);
    register_buses(interp);
    register_board(interp);
    register_device_info(interp);
}

fn register_gpio(interp: &mut Interpreter) {
    interp.register_native("RustNet.Hal.Gpio::SetMode(int,int)", |i, a| {
        let pin = arg_i32(i, a, 0)? as u32;
        let mode = arg_i32(i, a, 1)?;
        with_board(i, "Gpio.SetMode", |b| {
            let mode = pin_mode(mode)?;
            b.gpio(pin)?.set_mode(mode)
        })?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.Gpio::Write(int,bool)", |i, a| {
        let pin = arg_i32(i, a, 0)? as u32;
        let high = arg_bool(i, a, 1)?;
        with_board(i, "Gpio.Write", |b| b.gpio(pin)?.write(Level::from(high)))?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.Gpio::Read(int)", |i, a| {
        let pin = arg_i32(i, a, 0)? as u32;
        let level = with_board(i, "Gpio.Read", |b| b.gpio(pin)?.read())?;
        Ok(Some(Value::I32(i32::from(bool::from(level)))))
    });
    interp.register_native("RustNet.Hal.Gpio::Toggle(int)", |i, a| {
        let pin = arg_i32(i, a, 0)? as u32;
        with_board(i, "Gpio.Toggle", |b| b.gpio(pin)?.toggle())?;
        Ok(None)
    });
}

fn register_analog(interp: &mut Interpreter) {
    interp.register_native("RustNet.Hal.Adc::ReadRaw(int)", |i, a| {
        let channel = arg_i32(i, a, 0)? as u8;
        let raw = with_board(i, "Adc.ReadRaw", |b| b.adc(channel)?.read_raw())?;
        Ok(Some(Value::I32(raw as i32)))
    });
    interp.register_native("RustNet.Hal.Adc::ReadMillivolts(int)", |i, a| {
        let channel = arg_i32(i, a, 0)? as u8;
        let mv = with_board(i, "Adc.ReadMillivolts", |b| b.adc(channel)?.read_millivolts())?;
        Ok(Some(Value::I32(mv as i32)))
    });
    interp.register_native("RustNet.Hal.Pwm::Configure(int,int,int)", |i, a| {
        let channel = arg_i32(i, a, 0)? as u8;
        let hz = arg_i32(i, a, 1)? as u32;
        let duty = arg_i32(i, a, 2)?;
        with_board(i, "Pwm.Configure", |b| {
            if !(0..=10_000).contains(&duty) {
                return Err(HalError::InvalidArgument(
                    "duty is in hundredths of a percent, so 0..=10000",
                ));
            }
            let pwm = b.pwm(channel)?;
            pwm.set_frequency(hz)?;
            pwm.set_duty(duty as u16)?;
            pwm.enable()
        })?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.Pwm::Stop(int)", |i, a| {
        let channel = arg_i32(i, a, 0)? as u8;
        with_board(i, "Pwm.Stop", |b| b.pwm(channel)?.disable())?;
        Ok(None)
    });
}

fn register_buses(interp: &mut Interpreter) {
    interp.register_native("RustNet.Hal.I2c::Write(int,int,byte[])", |i, a| {
        let bus = arg_i32(i, a, 0)? as u8;
        let address = arg_i32(i, a, 1)? as u8;
        let data = arg_bytes(i, a, 2)?;
        with_board(i, "I2c.Write", |b| b.i2c(bus)?.write(address, &data))?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.I2c::Read(int,int,int)", |i, a| {
        let bus = arg_i32(i, a, 0)? as u8;
        let address = arg_i32(i, a, 1)? as u8;
        let length = arg_i32(i, a, 2)?.max(0) as usize;
        let mut buffer = vec![0u8; length];
        with_board(i, "I2c.Read", |b| b.i2c(bus)?.read(address, &mut buffer))?;
        let array = byte_array(i, &buffer);
        Ok(Some(Value::Obj(array)))
    });
    // Write-then-read without releasing the bus. A sensor register read is
    // exactly this, and splitting it into two calls lets another thread's
    // transfer land in the middle.
    interp.register_native("RustNet.Hal.I2c::WriteRead(int,int,byte[],int)", |i, a| {
        let bus = arg_i32(i, a, 0)? as u8;
        let address = arg_i32(i, a, 1)? as u8;
        let out = arg_bytes(i, a, 2)?;
        let length = arg_i32(i, a, 3)?.max(0) as usize;
        let mut buffer = vec![0u8; length];
        with_board(i, "I2c.WriteRead", |b| {
            b.i2c(bus)?.write_read(address, &out, &mut buffer)
        })?;
        let array = byte_array(i, &buffer);
        Ok(Some(Value::Obj(array)))
    });

    interp.register_native("RustNet.Hal.Spi::Transfer(int,byte[])", |i, a| {
        let bus = arg_i32(i, a, 0)? as u8;
        let out = arg_bytes(i, a, 1)?;
        let mut buffer = vec![0u8; out.len()];
        with_board(i, "Spi.Transfer", |b| b.spi(bus)?.transfer(&out, &mut buffer))?;
        let array = byte_array(i, &buffer);
        Ok(Some(Value::Obj(array)))
    });
    interp.register_native("RustNet.Hal.Spi::Write(int,byte[])", |i, a| {
        let bus = arg_i32(i, a, 0)? as u8;
        let out = arg_bytes(i, a, 1)?;
        with_board(i, "Spi.Write", |b| b.spi(bus)?.write(&out))?;
        Ok(None)
    });

    interp.register_native("RustNet.Hal.Uart::Configure(int,int)", |i, a| {
        let port = arg_i32(i, a, 0)? as u8;
        let baud = arg_i32(i, a, 1)? as u32;
        with_board(i, "Uart.Configure", |b| {
            // Only the baud is exposed: 8N1 is what every board here brings up
            // by default, and a program that needs 7E1 has a peripheral-specific
            // requirement that belongs in a driver, not in the generic API.
            let config = rustclr_hal::uart::UartConfig { baud, ..Default::default() };
            b.uart(port)?.configure(config)
        })?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.Uart::Write(int,byte[])", |i, a| {
        let port = arg_i32(i, a, 0)? as u8;
        let data = arg_bytes(i, a, 1)?;
        with_board(i, "Uart.Write", |b| b.uart(port)?.write(&data).map(|_| ()))?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.Uart::Read(int,int)", |i, a| {
        let port = arg_i32(i, a, 0)? as u8;
        let max = arg_i32(i, a, 1)?.max(0) as usize;
        let mut buffer = vec![0u8; max];
        let read = with_board(i, "Uart.Read", |b| b.uart(port)?.read(&mut buffer))?;
        let array = byte_array(i, &buffer[..read.min(max)]);
        Ok(Some(Value::Obj(array)))
    });
    interp.register_native("RustNet.Hal.Uart::Available(int)", |i, a| {
        let port = arg_i32(i, a, 0)? as u8;
        let n = with_board(i, "Uart.Available", |b| b.uart(port)?.bytes_available())?;
        Ok(Some(Value::I32(n as i32)))
    });
}

fn register_board(interp: &mut Interpreter) {
    interp.register_native("RustNet.Hal.Board::get_Name()", |i, _a| {
        let name = match i.board.as_ref() {
            Some(b) => b.name().to_string(),
            // Not an exception: asking *whether* there is a board is the one
            // question a program on any machine is entitled to ask.
            None => String::from("none"),
        };
        Ok(Some(string_value(i, &name)))
    });
    interp.register_native("RustNet.Hal.Board::get_IsPresent()", |i, _a| {
        Ok(Some(Value::I32(i32::from(i.board.is_some()))))
    });
    interp.register_native("RustNet.Hal.Delay::Micros(int)", |i, a| {
        let micros = arg_i32(i, a, 0)?.max(0) as u64;
        with_board(i, "Delay.Micros", |b| {
            b.delay().delay_us(micros);
            Ok(())
        })?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.Watchdog::Start(int)", |i, a| {
        let timeout = arg_i32(i, a, 0)?.max(0) as u32;
        with_board(i, "Watchdog.Start", |b| b.watchdog().start(timeout))?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.Watchdog::Feed()", |i, _a| {
        with_board(i, "Watchdog.Feed", |b| b.watchdog().feed())?;
        Ok(None)
    });
    interp.register_native("RustNet.Hal.Watchdog::Stop()", |i, _a| {
        with_board(i, "Watchdog.Stop", |b| b.watchdog().stop())?;
        Ok(None)
    });
}

/// `RustNet.Sys.DeviceInfo` — what this device is.
///
/// Answers the same questions `Board.Name` does, in the shape a telemetry
/// payload wants. The chip name is the *runtime's* view: it comes from the
/// board the firmware installed, not from a compile-time constant, so a build
/// running on the simulator says `host` rather than claiming the chip it was
/// cross-compiled for.
pub fn register_device_info(interp: &mut Interpreter) {
    /// A short chip identifier derived from the board's own name.
    fn chip_of(name: &str) -> &'static str {
        let lower = name.to_lowercase();
        if lower.contains("k210") || lower.contains("maix") {
            "k210"
        } else if lower.contains("c3") {
            "esp32c3"
        } else if lower.contains("esp32") || lower.contains("m5") {
            "esp32"
        } else if lower.contains("rp2040") || lower.contains("pico") {
            "rp2040"
        } else if lower.contains("stm32") || lower.contains("nucleo")
            || lower.contains("netduino") || lower.contains("meadow")
        {
            "stm32"
        } else {
            "host"
        }
    }

    interp.register_native("RustNet.Sys.DeviceInfo::Chip()", |i, _a| {
        let name = i.board.as_ref().map(|b| b.name().to_string()).unwrap_or_default();
        let chip = chip_of(&name).to_string();
        Ok(Some(string_value(i, &chip)))
    });
    interp.register_native("RustNet.Sys.DeviceInfo::Board()", |i, _a| {
        let name = match i.board.as_ref() {
            Some(b) => b.name().to_string(),
            None => String::from("none"),
        };
        Ok(Some(string_value(i, &name)))
    });
    interp.register_native("RustNet.Sys.DeviceInfo::Version()", |i, _a| {
        let version = String::from(env!("CARGO_PKG_VERSION"));
        Ok(Some(string_value(i, &version)))
    });
    interp.register_native("RustNet.Sys.DeviceInfo::UptimeMs()", |i, _a| {
        Ok(Some(Value::I64(i.host.monotonic_millis() as i64)))
    });
    interp.register_native("RustNet.Sys.Uptime::Ms()", |i, _a| {
        Ok(Some(Value::I64(i.host.monotonic_millis() as i64)))
    });
    interp.register_native("RustNet.Sys.DeviceInfo::Json()", |i, _a| {
        let name = match i.board.as_ref() {
            Some(b) => b.name().to_string(),
            None => String::from("none"),
        };
        let chip = chip_of(&name);
        let uptime = i.host.monotonic_millis();
        let version = env!("CARGO_PKG_VERSION");
        // Built here rather than concatenated differently in every program,
        // and escaped: a board name with a quote in it would otherwise produce
        // a payload a broker's consumer cannot parse.
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        let json = format!(
            "{{\"chip\":\"{chip}\",\"board\":\"{escaped}\",\"version\":\"{version}\",\"uptimeMs\":{uptime}}}"
        );
        Ok(Some(string_value(i, &json)))
    });
}
