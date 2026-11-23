//! Held-key reference counter and tap scheduler so overlapping controls never
//! leave stuck modifiers on Wayland/X11.

use crate::input::{InputEmitter, InputError};
use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

#[derive(Default)]
pub struct KeyScheduler {
    holds: BTreeMap<u8, Vec<u16>>,
    held: Vec<u16>,
}

impl KeyScheduler {
    pub fn press_control(
        &mut self,
        control_idx: u8,
        hid_codes: &[u16],
        emitter: &mut dyn InputEmitter,
    ) -> Result<(), InputError> {
        if self.holds.contains_key(&control_idx) {
            return Ok(());
        }
        let keys = hid_codes.to_vec();
        self.holds.insert(control_idx, keys.clone());
        for &code in &keys {
            if code == 0x105 || code == 0x106 {
                // Wheel pulse — emit immediately, not held
                emitter.emit_hid(code, true)?;
                continue;
            }
            if !self.held.contains(&code) {
                self.held.push(code);
                emitter.emit_hid(code, true)?;
            }
        }
        Ok(())
    }

    pub fn release_control(
        &mut self,
        control_idx: u8,
        emitter: &mut dyn InputEmitter,
    ) -> Result<(), InputError> {
        self.holds.remove(&control_idx);
        self.release_unused(emitter)
    }

    /// Taps a chord cleanly (press modifiers -> key -> small hold -> release in reverse).
    pub fn tap_combo(
        &mut self,
        hid_codes: &[u16],
        emitter: &mut dyn InputEmitter,
    ) -> Result<(), InputError> {
        for &code in hid_codes {
            emitter.emit_hid(code, true)?;
        }
        let has_non_wheel = hid_codes.iter().any(|&c| c != 0x105 && c != 0x106);
        if has_non_wheel {
            thread::sleep(Duration::from_millis(25));
            for &code in hid_codes.iter().rev() {
                if code != 0x105 && code != 0x106 {
                    emitter.emit_hid(code, false)?;
                }
            }
        }
        Ok(())
    }

    pub fn release_all(&mut self, emitter: &mut dyn InputEmitter) -> Result<(), InputError> {
        self.holds.clear();
        self.release_unused(emitter)
    }

    pub fn held_keys(&self) -> &[u16] {
        &self.held
    }

    fn is_needed(&self, code: u16) -> bool {
        self.holds.values().any(|keys| keys.contains(&code))
    }

    fn release_unused(&mut self, emitter: &mut dyn InputEmitter) -> Result<(), InputError> {
        let mut last_err = None;
        for i in (0..self.held.len()).rev() {
            let code = self.held[i];
            if self.is_needed(code) {
                continue;
            }
            match emitter.emit_hid(code, false) {
                Ok(()) => {
                    self.held.remove(i);
                }
                Err(e) => last_err = Some(e),
            }
        }
        last_err.map_or(Ok(()), Err)
    }
}
