use rustclr_hal::gpio::{Edge, GpioPin, Level, PinMode};
use rustclr_hal::{HalError, HalResult};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct PinState {
    pub level: bool,
    pub mode: Option<PinMode>,
    pub callback: Option<(Edge, Box<dyn FnMut(Level) + Send>)>,
}

/// A simulated GPIO pin backed by shared state so tests can drive inputs.
pub struct SimGpioPin {
    pub(crate) state: Arc<Mutex<PinState>>,
}

impl SimGpioPin {
    pub(crate) fn new() -> Self {
        Self { state: Arc::new(Mutex::new(PinState::default())) }
    }

    /// Externally drive the pin (as if a wire changed level) and fire edge
    /// callbacks that match the transition.
    pub(crate) fn drive(&self, level: Level) {
        let mut st = self.state.lock().unwrap();
        let old = st.level;
        let new: bool = level.into();
        st.level = new;
        if old == new {
            return;
        }
        let rising = !old && new;
        if let Some((edge, cb)) = st.callback.as_mut() {
            let fire = matches!(
                (edge, rising),
                (Edge::Rising, true) | (Edge::Falling, false) | (Edge::Both, _)
            );
            if fire {
                cb(level);
            }
        }
    }
}

impl GpioPin for SimGpioPin {
    fn set_mode(&mut self, mode: PinMode) -> HalResult<()> {
        self.state.lock().unwrap().mode = Some(mode);
        Ok(())
    }

    fn write(&mut self, level: Level) -> HalResult<()> {
        let mut st = self.state.lock().unwrap();
        match st.mode {
            Some(PinMode::Output) | Some(PinMode::OutputOpenDrain) => {
                st.level = level.into();
                Ok(())
            }
            _ => Err(HalError::InvalidArgument("pin not in output mode")),
        }
    }

    fn read(&mut self) -> HalResult<Level> {
        Ok(self.state.lock().unwrap().level.into())
    }

    fn on_edge(&mut self, edge: Edge, callback: Box<dyn FnMut(Level) + Send>) -> HalResult<()> {
        self.state.lock().unwrap().callback = Some((edge, callback));
        Ok(())
    }

    fn clear_interrupt(&mut self) -> HalResult<()> {
        self.state.lock().unwrap().callback = None;
        Ok(())
    }
}
