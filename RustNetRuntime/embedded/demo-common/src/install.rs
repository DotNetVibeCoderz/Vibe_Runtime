//! Attaching the device service before anything runs.
//!
//! Split out from [`crate::appserver`] because of *when* it happens. The
//! serving loop runs after the firmware's embedded program returns — and a
//! device program loops forever, so for the boards this actually ships on,
//! that is never.
//!
//! A board therefore has to be reachable **while** its program runs, not
//! after. This installs the service before the first instruction, so the very
//! first thing a firmware executes is already answering host tools; the loop
//! afterwards takes the same service back and carries on with it, keeping the
//! log and the uptime across the boundary.
//!
//! That ordering was discovered the direct way: with the service installed
//! only by the serving loop, a Maix Go running the `Showcase` sample printed
//! no RNDP banner at all and answered nothing, because `Showcase` never
//! returns.

use alloc::boxed::Box;
use alloc::string::ToString;

use rustclr_core::Interpreter;

use crate::rndp::{attach, DeviceService, STORAGE_WINDOW};

/// Put the service on the interpreter, and say what the board can offer.
///
/// Does nothing, and reports why, on a board whose HAL cannot answer — so a
/// firmware that could not do this before behaves exactly as it did.
pub fn install<W: core::fmt::Write>(out: &mut W, interp: &mut Interpreter) {
    let (capability, board_name) = {
        let Some(board) = interp.board.as_mut() else { return };
        (DeviceService::capability(board.as_mut()), board.name().to_string())
    };

    if !capability.console {
        // Said once, plainly. A board that cannot answer should not leave
        // somebody wondering why the Workbench times out.
        let _ = writeln!(
            out,
            "rndp             unavailable: {}",
            capability.shortfall().unwrap_or("")
        );
        return;
    }

    let _ = writeln!(
        out,
        "rndp             listening on the console, protocol v{}",
        rustclr_rndp::PROTOCOL_VERSION
    );
    match capability.shortfall() {
        Some(missing) => {
            let _ = writeln!(out, "                 limited: {missing}");
        }
        None => {
            let apps = interp
                .board
                .as_mut()
                .and_then(|b| b.extmem(STORAGE_WINDOW).ok())
                .and_then(|flash| rustclr_device::store::list_apps(flash).ok())
                .map(|a| a.len())
                .unwrap_or(0);
            let _ = writeln!(out, "                 {apps} application(s) stored");
        }
    }

    attach(interp, Box::new(DeviceService::new(crate::appserver::chip_name(), &board_name)));
}
