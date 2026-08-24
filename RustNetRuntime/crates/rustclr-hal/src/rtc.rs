use crate::HalResult;

/// Calendar time as kept by a battery-backed RTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Seconds since the Unix epoch (proleptic Gregorian, UTC).
    pub fn to_epoch(&self) -> u64 {
        let days = days_from_civil(self.year as i64, self.month as i64, self.day as i64);
        (days * 86_400 + self.hour as i64 * 3_600 + self.minute as i64 * 60 + self.second as i64)
            .max(0) as u64
    }

    pub fn from_epoch(epoch: u64) -> DateTime {
        let days = (epoch / 86_400) as i64;
        let rem = epoch % 86_400;
        let (y, m, d) = civil_from_days(days);
        DateTime {
            year: y as u16,
            month: m as u8,
            day: d as u8,
            hour: (rem / 3_600) as u8,
            minute: (rem % 3_600 / 60) as u8,
            second: (rem % 60) as u8,
        }
    }
}

// Howard Hinnant's civil-days algorithms.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Battery-backed real-time clock with an alarm usable as a wake source.
pub trait Rtc: Send {
    fn now(&mut self) -> HalResult<DateTime>;
    fn set(&mut self, dt: DateTime) -> HalResult<()>;
    fn epoch(&mut self) -> HalResult<u64> {
        Ok(self.now()?.to_epoch())
    }
    /// Arm the alarm at an absolute epoch second (deep-sleep wake source).
    fn set_alarm(&mut self, epoch: u64) -> HalResult<()>;
    fn clear_alarm(&mut self) -> HalResult<()>;
    fn alarm(&self) -> Option<u64>;
}
