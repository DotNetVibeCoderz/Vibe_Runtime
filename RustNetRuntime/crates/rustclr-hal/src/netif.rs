
#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};
use crate::HalResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetIfKind {
    Wifi,
    Ethernet,
    /// Point-to-point over a serial modem link.
    Ppp,
    /// LTE/NB-IoT module (PPP or vendor AT data mode underneath).
    Cellular,
}

impl NetIfKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            NetIfKind::Wifi => "wifi",
            NetIfKind::Ethernet => "ethernet",
            NetIfKind::Ppp => "ppp",
            NetIfKind::Cellular => "cellular",
        }
    }
}

/// Bring-up parameters. Unused fields stay empty (DHCP is the default for
/// Ethernet; APN only applies to cellular; the serial port only to PPP).
#[derive(Debug, Clone, Default)]
pub struct NetIfConfig {
    /// Static IPv4 "a.b.c.d" or empty for DHCP.
    pub static_ip: String,
    pub gateway: String,
    /// WiFi network name; empty for every other interface kind. The PSK
    /// travels in `password`.
    pub ssid: String,
    /// Cellular APN, e.g. "internet".
    pub apn: String,
    pub username: String,
    pub password: String,
    /// UART port for PPP/cellular modems.
    pub uart_port: u8,
}

#[derive(Debug, Clone, Default)]
pub struct NetIfStatus {
    pub up: bool,
    pub ip: String,
    pub gateway: String,
    pub mac: String,
    /// Signal strength in dBm (cellular/wifi), 0 when not applicable.
    pub rssi_dbm: i32,
    /// Network operator name (cellular), empty otherwise.
    pub operator_name: String,
    /// Associated WiFi network name, empty otherwise. Reported by the
    /// interface itself, so it reflects the join that actually happened —
    /// which on some targets is performed by the firmware, not by this HAL.
    pub ssid: String,
}

/// One network interface. The runtime's socket layer routes over whichever
/// interface is up (device default-route semantics).
pub trait NetInterface: Send {
    fn kind(&self) -> NetIfKind;
    fn bring_up(&mut self, config: &NetIfConfig) -> HalResult<()>;
    fn bring_down(&mut self) -> HalResult<()>;
    fn status(&mut self) -> HalResult<NetIfStatus>;
    fn is_up(&mut self) -> bool {
        self.status().map(|s| s.up).unwrap_or(false)
    }
}
