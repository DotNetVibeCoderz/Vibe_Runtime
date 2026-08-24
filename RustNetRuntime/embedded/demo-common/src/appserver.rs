//! Staying reachable: answering host tools, and running what they send.
//!
//! [`crate::rndp`] is the service; this is the loop around it. Called once the
//! demo program has finished, with the interpreter it ran in.
//!
//! # The shape of the loop
//!
//! A device alternates between two states, and both have to answer the host:
//!
//! * **running an application**, where the service is polled from inside the
//!   interpreter's instruction loop;
//! * **idle**, where there is no instruction loop, so the same servicing is
//!   driven directly from here.
//!
//! Both go through [`crate::rndp::DeviceService::service_once`], so a command
//! cannot be answered one way while a program runs and another way while the
//! board sits idle.
//!
//! # A board that cannot answer behaves exactly as it did before
//!
//! [`serve`] returns immediately when the HAL has no readable console UART.
//! Several chip HALs here do not yet, and a firmware for one of those must not
//! start behaving differently because this module exists.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};

use rustclr_core::Interpreter;
use rustclr_metadata::Image;

use crate::rndp::{attach, detach, DeviceService, Pending, STORAGE_WINDOW};
use crate::EXEC_SLOTS;

/// Stay reachable: answer host tools, and run the applications they send.
pub fn serve<W: core::fmt::Write>(out: &mut W, interp: &mut Interpreter, libraries: &[&[u8]]) {
    // The service was installed before the embedded program ran, by
    // [`crate::install::install`]. Taking it back rather than building a new
    // one keeps the log and the uptime continuous across the boundary --
    // starting the next application must not look like a reboot to whoever is
    // watching. Nothing to take means the board could not answer in the first
    // place, and it has already said so.
    let Some(mut service) = detach(interp) else { return };
    service.set_running(None);

    // What to run first: whatever was set to autostart, if it is still there.
    let mut next = interp
        .board
        .as_mut()
        .and_then(|b| b.extmem(STORAGE_WINDOW).ok())
        .and_then(rustclr_device::store::autostart);

    loop {
        if let Some(name) = next.take() {
            let _ = writeln!(out, "rndp             starting `{name}`");
            service.set_running(Some(&name));
            let outcome = run_stored_app(interp, &name, libraries, &mut service);
            service.set_running(None);
            match outcome {
                Ok(code) => {
                    let _ = writeln!(out, "rndp             `{name}` exited with {code}");
                }
                // A stop is what was asked for, not a failure. Reporting it as
                // one would put an error on the console every time somebody
                // pressed the stop button.
                Err(e) if e.contains("external request") => {
                    let _ = writeln!(out, "rndp             `{name}` stopped");
                }
                Err(e) => {
                    let _ = writeln!(out, "rndp             `{name}` failed: {e}");
                    service.log_line(&e);
                }
            }
        } else {
            // Idle: nothing is running, so nothing is polling the service from
            // an instruction loop. Drive it from here instead.
            service.service_once(interp);
        }

        match service.take_pending() {
            Pending::Start(name) => next = Some(name),
            Pending::Stop | Pending::None => {}
            Pending::Reboot => {
                let _ = writeln!(out, "rndp             rebooting");
                if let Some(board) = interp.board.as_mut() {
                    board.power().reset();
                }
                // Reached only if `reset` returned, which means the HAL cannot
                // restart this part. Said rather than silently carried on.
                let _ = writeln!(out, "rndp             this board cannot restart itself");
            }
        }
    }
}

/// Load one stored application and run it to completion.
///
/// A **fresh interpreter** per application, sharing only the board. Reusing one
/// would carry the previous program's statics, its heap and its loaded types
/// into the next: a second `START_APP` of the same assembly would find its
/// types already registered and fail, and two applications would be able to
/// see each other's static state, which is not what running an application
/// means.
fn run_stored_app(
    interp: &mut Interpreter,
    name: &str,
    libraries: &[&[u8]],
    service: &mut Box<DeviceService>,
) -> Result<i32, String> {
    let image = {
        let board = interp.board.as_mut().ok_or_else(|| "no board".to_string())?;
        let flash = board.extmem(STORAGE_WINDOW).map_err(|e| format!("{e:?}"))?;
        rustclr_device::store::read_app(flash, name)?
    };

    // The board moves across to the new interpreter and back again, so the
    // application reaches the same hardware the demo did.
    let mut app = Interpreter::with_host(Box::new(rustclr_core::CaptureHost::new()));
    rustclr_bcl::install(&mut app);
    app.heap = rustclr_gc::Heap::embedded(EXEC_SLOTS).into();
    app.board = interp.board.take();

    for library in libraries {
        if let Ok(image) = Image::from_bytes(library.to_vec()) {
            let _ = app.loader.load_image(image);
        }
    }

    let outcome = load_and_run(&mut app, image, service);

    // What the application printed, folded into the log the host reads.
    //
    // It ran against a `CaptureHost`, which buffers rather than streaming: the
    // board's console belongs to the firmware, and handing it to an
    // application would mean two owners of one UART with a protocol on it.
    // The cost is that this only arrives when the application *returns* --
    // a program that loops forever contributes nothing here, and `GET_LOGS`
    // then shows the firmware's own events and no more. Said plainly rather
    // than left for somebody to discover from an empty pane.
    if let Some(text) = app.host.captured_output() {
        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            if !line.is_empty() {
                service.log_line(line);
            }
        }
    }

    // Whatever happened, the board comes back: the firmware's own reporting
    // and the next application both need it.
    interp.board = app.board.take();
    outcome
}

/// Load the image into `app`, hand it the service, run it, take the service back.
///
/// The service is moved rather than shared because the interpreter owns its
/// poll hook. It has to come back afterwards: the log, the uptime and the
/// stored-application list belong to the *board*, and starting the next
/// application must not look like a reboot to whoever is watching.
fn load_and_run(
    app: &mut Interpreter,
    image: alloc::vec::Vec<u8>,
    service: &mut Box<DeviceService>,
) -> Result<i32, String> {
    let image = Image::from_bytes(image).map_err(|e| format!("{e}"))?;
    let id = app.loader.load_image(image).map_err(|e| format!("{e}"))?;

    let borrowed = core::mem::replace(service, Box::new(DeviceService::new("", "")));
    attach(app, borrowed);
    let outcome = app.run_entry_point(id).map_err(|e| format!("{e}"));
    if let Some(back) = detach(app) {
        *service = back;
    }
    outcome
}

/// The chip this firmware was built for.
///
/// Derived from the target rather than repeated as a string in each firmware,
/// so it cannot disagree with what was actually compiled.
pub(crate) const fn chip_name() -> &'static str {
    if cfg!(target_arch = "riscv64") {
        "k210"
    } else if cfg!(target_arch = "xtensa") {
        "esp32"
    } else if cfg!(target_arch = "riscv32") {
        "esp32c3"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "host"
    }
}
