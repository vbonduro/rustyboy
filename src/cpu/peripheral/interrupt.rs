use std::cell::RefCell;
use std::rc::Rc;

use crate::memory::memory::IoDevice;

const IF_ADDR: u16 = 0xFF0F;
const IE_ADDR: u16 = 0xFFFF;

/// Owns the Game Boy interrupt enable (IE) and interrupt flag (IF) registers.
/// Peripherals call `request(bit)` to raise an interrupt. The CPU calls
/// `take_pending()` to atomically consume the highest-priority pending interrupt,
/// or `has_pending()` for a non-destructive check (used by HALT wake logic).
pub struct InterruptController {
    ie: u8,
    if_: u8,
}

impl InterruptController {
    pub fn new() -> Self {
        Self { ie: 0, if_: 0 }
    }

    pub fn ie(&self) -> u8 {
        self.ie
    }

    pub fn set_ie(&mut self, value: u8) {
        self.ie = value;
    }

    pub fn if_reg(&self) -> u8 {
        self.if_
    }

    pub fn set_if(&mut self, value: u8) {
        self.if_ = value;
    }

    /// Set an IF bit — called by peripherals (Timer, VBlank, etc.) to request an interrupt.
    pub fn request(&mut self, bit: u8) {
        self.if_ |= 1 << bit;
    }

    /// Non-destructive check: is there any pending interrupt (IE & IF != 0)?
    /// Used by HALT wake logic — does not clear IF.
    pub fn has_pending(&self) -> bool {
        self.ie & self.if_ != 0
    }

    /// Atomically returns the lowest pending interrupt bit position and clears it from IF.
    /// Returns None if no interrupt is pending.
    pub fn take_pending(&mut self) -> Option<u8> {
        let pending = self.ie & self.if_;
        if pending == 0 {
            return None;
        }
        let bit = pending.trailing_zeros() as u8;
        self.if_ &= !(1 << bit);
        Some(bit)
    }
}

/// IoDevice wrapper so an `Rc<RefCell<InterruptController>>` can be registered
/// on GameBoyMemory. Register one for `0xFF0F..=0xFF0F` and another for
/// `0xFFFF..=0xFFFF`.
pub struct SharedInterruptDevice(pub Rc<RefCell<InterruptController>>);

impl IoDevice for SharedInterruptDevice {
    fn read(&self, address: u16) -> u8 {
        let ic = self.0.borrow();
        match address {
            IF_ADDR => ic.if_reg(),
            IE_ADDR => ic.ie(),
            _ => 0xFF,
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        let mut ic = self.0.borrow_mut();
        match address {
            IF_ADDR => ic.set_if(value),
            IE_ADDR => ic.set_ie(value),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_pending_when_empty() {
        let ic = InterruptController::new();
        assert!(!ic.has_pending());
        assert!(InterruptController::new().take_pending().is_none());
    }

    #[test]
    fn test_request_sets_pending() {
        let mut ic = InterruptController::new();
        ic.set_ie(0xFF); // all enabled
        ic.request(0);
        assert!(ic.has_pending());
    }

    #[test]
    fn test_take_pending_returns_lowest_bit_and_clears_it() {
        let mut ic = InterruptController::new();
        ic.set_ie(0xFF);
        ic.request(0);
        ic.request(2);
        assert_eq!(ic.take_pending(), Some(0));
        assert_eq!(ic.take_pending(), Some(2));
        assert_eq!(ic.take_pending(), None);
    }

    #[test]
    fn test_has_pending_is_non_destructive() {
        let mut ic = InterruptController::new();
        ic.set_ie(0xFF);
        ic.request(1);
        assert!(ic.has_pending());
        assert!(ic.has_pending()); // still pending
    }

    #[test]
    fn test_pending_requires_ie_enabled() {
        let mut ic = InterruptController::new();
        ic.set_ie(0x00); // all disabled
        ic.request(0);
        assert!(!ic.has_pending());
        assert!(ic.take_pending().is_none());
    }
}
