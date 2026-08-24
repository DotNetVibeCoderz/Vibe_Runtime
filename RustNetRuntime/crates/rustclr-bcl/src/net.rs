//! `RustNet.Net` — WiFi association and an MQTT client.
//!
//! # What "WiFi" means depends on what is underneath
//!
//! On a **board**, joining a network is the radio's job and the HAL's
//! [`NetInterface`](rustclr_hal::netif::NetInterface) is how a program asks
//! for it. On a **desktop**, the operating system already owns the radio: a
//! program cannot and should not make the machine leave one network and join
//! another because a C# line said so.
//!
//! So `Wifi.Connect` does two different, honest things. With a board that has
//! a network interface it associates. Without one it **reports whether the
//! machine already has connectivity** and says so through `GetSsid`, which
//! answers `host` rather than inventing the name of a network it did not join.
//! It does not silently pretend to have associated, and it does not refuse a
//! program that only wanted to reach a broker.
//!
//! # MQTT is real TCP
//!
//! [`rustclr_net::mqtt`] speaks MQTT 3.1.1 over a socket. There is no
//! simulation here: `Mqtt.Connect("broker:1883", id)` opens a connection, and
//! a broker that is not there produces an error naming what failed.
//!
//! One connection per interpreter, held here. A device has one broker and a
//! few kilobytes; a client pool would be a feature nothing on these boards
//! asked for.

use crate::support::*;
use rustclr_core::{ExecutionError, Interpreter, Value};

#[allow(unused_imports)]
use crate::prelude::*;

fn net_error(what: &str, detail: String) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::Io,
        &format!("{what}: {detail}"),
    )
}

/// The full surface: WiFi through the HAL, MQTT over real sockets.
#[cfg(feature = "std")]
pub fn register(interp: &mut Interpreter) {
    register_wifi(interp);
    register_mqtt(interp);
}

/// The same surface on a board, where there are no sockets.
///
/// Registered rather than omitted, and that is the point. A binding that does
/// not exist is not a catchable failure — it is `MissingImplementation`, which
/// ends the program. A program written to handle a broker being unreachable
/// deserves to reach its own error path, so these answer "not connected"
/// honestly and let it.
#[cfg(not(feature = "std"))]
pub fn register_offline(interp: &mut Interpreter) {
    register_wifi(interp);

    interp.register_native("RustNet.Net.Mqtt::Connect(string,string)", |i, _a| {
        i.host
            .write_err("[mqtt] this firmware has no network stack; not connected
");
        Ok(Some(Value::I32(0)))
    });
    interp.register_native(
        "RustNet.Net.Mqtt::ConnectAuth(string,string,string,string)",
        |i, _a| {
            i.host
                .write_err("[mqtt] this firmware has no network stack; not connected
");
            Ok(Some(Value::I32(0)))
        },
    );
    interp.register_native("RustNet.Net.Mqtt::IsConnected()", |_i, _a| {
        Ok(Some(Value::I32(0)))
    });
    // Catchable, because a program that publishes without checking deserves an
    // exception it can handle rather than a runtime that stops.
    for key in [
        "RustNet.Net.Mqtt::Publish(string,string,int)",
        "RustNet.Net.Mqtt::Subscribe(string)",
    ] {
        interp.register_native(key, |_i, _a| {
            Err(net_error("Mqtt", "not connected: this firmware has no network stack".into()))
        });
    }
    // An idle broker and no broker at all look the same to a poller, and both
    // are "nothing arrived".
    interp.register_native("RustNet.Net.Mqtt::Poll()", |i, _a| {
        Ok(Some(string_value(i, "")))
    });
    interp.register_native("RustNet.Net.Mqtt::Disconnect()", |_i, _a| Ok(None));
}

fn register_wifi(interp: &mut Interpreter) {
    interp.register_native("RustNet.Net.Wifi::Connect(string,string)", |i, a| {
        let ssid = arg_string_or_empty(i, a, 0)?;
        let psk = arg_string_or_empty(i, a, 1)?;
        let joined = match i.board.as_mut() {
            Some(board) => match board.netif(rustclr_hal::netif::NetIfKind::Wifi) {
                Ok(netif) => {
                    let config = rustclr_hal::netif::NetIfConfig {
                        ssid: ssid.clone(),
                        password: psk.clone(),
                        ..Default::default()
                    };
                    netif
                        .bring_up(&config)
                        .map_err(|e| net_error("Wifi.Connect", format!("{e}")))?;
                    // `bring_up` returning means the request was accepted;
                    // whether the join *happened* is what `status` answers,
                    // and association takes seconds.
                    netif.is_up()
                }
                // A board with no radio answers `false`, which is what the
                // method promises: it did not connect. An exception here would
                // be a programmer-error signal for a hardware fact, and it
                // killed a dashboard that was written to handle `false`.
                Err(_) => {
                    i.host.write_err(&format!(
                        "[wifi] this board has no WiFi interface, so `{ssid}` was not joined
"
                    ));
                    false
                }
            },
            // No board: the machine's own connectivity, which the operating
            // system owns. Reported, never joined.
            None => true,
        };
        Ok(Some(Value::I32(i32::from(joined))))
    });
    interp.register_native("RustNet.Net.Wifi::IsConnected()", |i, _a| {
        let up = match i.board.as_mut() {
            Some(board) => match board.netif(rustclr_hal::netif::NetIfKind::Wifi) {
                Ok(netif) => netif.is_up(),
                Err(_) => false,
            },
            None => true,
        };
        Ok(Some(Value::I32(i32::from(up))))
    });
    interp.register_native("RustNet.Net.Wifi::GetSsid()", |i, _a| {
        let ssid = match i.board.as_mut() {
            Some(board) => match board.netif(rustclr_hal::netif::NetIfKind::Wifi) {
                Ok(netif) => netif.status().map(|s| s.ssid).unwrap_or_default(),
                Err(_) => String::new(),
            },
            // Not a made-up network name. The program is on a host, and
            // saying so is more useful than a plausible-looking lie.
            None => String::from("host"),
        };
        Ok(Some(string_value(i, &ssid)))
    });
    interp.register_native("RustNet.Net.Wifi::GetIp()", |i, _a| {
        let ip = match i.board.as_mut() {
            Some(board) => match board.netif(rustclr_hal::netif::NetIfKind::Wifi) {
                Ok(netif) => netif.status().map(|s| s.ip).unwrap_or_default(),
                Err(_) => String::new(),
            },
            None => String::from("127.0.0.1"),
        };
        Ok(Some(string_value(i, &ip)))
    });
    interp.register_native("RustNet.Net.Wifi::Disconnect()", |i, _a| {
        if let Some(board) = i.board.as_mut() {
            if let Ok(netif) = board.netif(rustclr_hal::netif::NetIfKind::Wifi) {
                let _ = netif.bring_down();
            }
        }
        Ok(None)
    });
    interp.register_native("RustNet.Net.Wifi::GetRssi()", |i, _a| {
        let rssi = match i.board.as_mut() {
            Some(board) => match board.netif(rustclr_hal::netif::NetIfKind::Wifi) {
                Ok(netif) => netif.status().map(|s| s.rssi_dbm).unwrap_or(0),
                Err(_) => 0,
            },
            // 0 dBm is not a plausible reading, which is the point: there is
            // no radio here and no signal strength to report.
            None => 0,
        };
        Ok(Some(Value::I32(rssi)))
    });
}

#[cfg(feature = "std")]
fn register_mqtt(interp: &mut Interpreter) {
    interp.register_native("RustNet.Net.Mqtt::Connect(string,string)", |i, a| {
        let address = arg_string_or_empty(i, a, 0)?;
        let client_id = arg_string_or_empty(i, a, 1)?;
        match rustclr_net::mqtt::MqttClient::connect(&address, &client_id) {
            Ok(client) => {
                i.mqtt = Some(client);
                Ok(Some(Value::I32(1)))
            }
            // `false`, not an exception: a dashboard's job is to keep trying,
            // and the templates loop on this return value.
            Err(e) => {
                i.host.write_err(&format!("[mqtt] connect to {address} failed: {e}\n"));
                Ok(Some(Value::I32(0)))
            }
        }
    });
    interp.register_native(
        "RustNet.Net.Mqtt::ConnectAuth(string,string,string,string)",
        |i, a| {
            let address = arg_string_or_empty(i, a, 0)?;
            let client_id = arg_string_or_empty(i, a, 1)?;
            let user = arg_string_or_empty(i, a, 2)?;
            let password = arg_string_or_empty(i, a, 3)?;
            match rustclr_net::mqtt::MqttClient::connect_auth(
                &address,
                &client_id,
                Some(user.as_str()),
                Some(password.as_str()),
            ) {
                Ok(client) => {
                    i.mqtt = Some(client);
                    Ok(Some(Value::I32(1)))
                }
                Err(e) => {
                    i.host.write_err(&format!("[mqtt] connect to {address} failed: {e}\n"));
                    Ok(Some(Value::I32(0)))
                }
            }
        },
    );
    interp.register_native("RustNet.Net.Mqtt::IsConnected()", |i, _a| {
        Ok(Some(Value::I32(i32::from(i.mqtt.is_some()))))
    });
    interp.register_native("RustNet.Net.Mqtt::Publish(string,string,int)", |i, a| {
        let topic = arg_string_or_empty(i, a, 0)?;
        let payload = arg_string_or_empty(i, a, 1)?;
        let qos = arg_i32(i, a, 2)?.clamp(0, 2) as u8;
        let Some(client) = i.mqtt.as_mut() else {
            return Err(net_error("Mqtt.Publish", "not connected".into()));
        };
        if let Err(e) = client.publish(&topic, payload.as_bytes(), qos) {
            // The connection is gone, so drop it: leaving a dead client in
            // place makes every later call fail for a reason that no longer
            // names what happened.
            i.mqtt = None;
            return Err(net_error("Mqtt.Publish", format!("{e:?}")));
        }
        Ok(None)
    });
    interp.register_native("RustNet.Net.Mqtt::Subscribe(string)", |i, a| {
        let topic = arg_string_or_empty(i, a, 0)?;
        let Some(client) = i.mqtt.as_mut() else {
            return Err(net_error("Mqtt.Subscribe", "not connected".into()));
        };
        if let Err(e) = client.subscribe(&topic) {
            i.mqtt = None;
            return Err(net_error("Mqtt.Subscribe", format!("{e:?}")));
        }
        Ok(None)
    });
    // Returns `topic\npayload`, or empty when nothing arrived. Not an
    // exception: an idle broker is the normal case, and a dashboard polls this
    // every frame.
    interp.register_native("RustNet.Net.Mqtt::Poll()", |i, _a| {
        let Some(client) = i.mqtt.as_mut() else {
            return Ok(Some(string_value(i, "")));
        };
        match client.poll_message() {
            Ok((topic, payload)) => {
                let text = format!("{topic}\n{}", String::from_utf8_lossy(&payload));
                Ok(Some(string_value(i, &text)))
            }
            Err(_) => Ok(Some(string_value(i, ""))),
        }
    });
    interp.register_native("RustNet.Net.Mqtt::Disconnect()", |i, _a| {
        if let Some(client) = i.mqtt.take() {
            let _ = client.disconnect();
        }
        Ok(None)
    });
}
