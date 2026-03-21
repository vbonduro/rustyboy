use std::cell::RefCell;
use std::rc::Rc;

use crate::cpu::peripheral::bus::Peripheral;
use crate::memory::memory::{BusEvent, Memory};

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

/// Newtype wrapper so an `Rc<RefCell<InterruptController>>` can be subscribed to the
/// peripheral bus. Subscribe one instance to `0xFF0F..=0xFF0F` and another to
/// `0xFFFF..=0xFFFF`.
pub struct SharedInterruptController(pub Rc<RefCell<InterruptController>>);

impl Peripheral for SharedInterruptController {
    fn handle(&mut self, event: &BusEvent, _mem: &mut dyn Memory) {
        let mut ic = self.0.borrow_mut();
        match event.address {
            IF_ADDR => ic.if_ = event.value,
            IE_ADDR => ic.ie = event.value,
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
        ic.ie = 0xFF; // all enabled
        ic.request(0);
        assert!(ic.has_pending());
    }

    #[test]
    fn test_take_pending_returns_lowest_bit_and_clears_it() {
        let mut ic = InterruptController::new();
        ic.ie = 0xFF;
        ic.request(0);
        ic.request(2);
        assert_eq!(ic.take_pending(), Some(0));
        assert_eq!(ic.take_pending(), Some(2));
        assert_eq!(ic.take_pending(), None);
    }

    #[test]
    fn test_has_pending_is_non_destructive() {
        let mut ic = InterruptController::new();
        ic.ie = 0xFF;
        ic.request(1);
        assert!(ic.has_pending());
        assert!(ic.has_pending()); // still pending
    }

    #[test]
    fn test_pending_requires_ie_enabled() {
        let mut ic = InterruptController::new();
        ic.ie = 0x00; // all disabled
        ic.request(0);
        assert!(!ic.has_pending());
        assert!(ic.take_pending().is_none());
    }
}
