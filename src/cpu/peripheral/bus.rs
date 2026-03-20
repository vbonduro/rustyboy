use std::ops::RangeInclusive;

use crate::memory::memory::{BusEvent, Memory};

pub trait Peripheral {
    fn handle(&mut self, event: &BusEvent, mem: &mut dyn Memory);
}

pub struct PeripheralBus {
    subscriptions: Vec<(RangeInclusive<u16>, Box<dyn Peripheral>)>,
}

impl PeripheralBus {
    pub fn new() -> Self {
        Self {
            subscriptions: Vec::new(),
        }
    }

    pub fn subscribe(&mut self, range: RangeInclusive<u16>, peripheral: Box<dyn Peripheral>) {
        self.subscriptions.push((range, peripheral));
    }

    pub fn flush(&mut self, mem: &mut dyn Memory) {
        let events = mem.drain_events();
        for event in &events {
            for (range, peripheral) in &mut self.subscriptions {
                if range.contains(&event.address) {
                    peripheral.handle(event, mem);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::memory::GameBoyMemory;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct SpyPeripheral {
        received: Rc<RefCell<Vec<BusEvent>>>,
    }

    impl SpyPeripheral {
        fn new(received: Rc<RefCell<Vec<BusEvent>>>) -> Self {
            Self { received }
        }
    }

    impl Peripheral for SpyPeripheral {
        fn handle(&mut self, event: &BusEvent, _mem: &mut dyn Memory) {
            self.received.borrow_mut().push(event.clone());
        }
    }

    #[test]
    fn test_subscriber_receives_event_in_range() {
        let mut mem = GameBoyMemory::new();
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut bus = PeripheralBus::new();
        bus.subscribe(
            0xFF01..=0xFF02,
            Box::new(SpyPeripheral::new(received.clone())),
        );

        mem.write(0xFF01, 0x48).unwrap();
        bus.flush(&mut mem);

        let events = received.borrow();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].address, 0xFF01);
        assert_eq!(events[0].value, 0x48);
    }

    #[test]
    fn test_subscriber_does_not_receive_event_outside_range() {
        let mut mem = GameBoyMemory::new();
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut bus = PeripheralBus::new();
        bus.subscribe(
            0xFF01..=0xFF02,
            Box::new(SpyPeripheral::new(received.clone())),
        );

        mem.write(0xFF03, 0x99).unwrap(); // outside range
        bus.flush(&mut mem);

        assert_eq!(received.borrow().len(), 0);
    }

    #[test]
    fn test_flush_clears_events_so_second_flush_sees_only_new_writes() {
        let mut mem = GameBoyMemory::new();
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut bus = PeripheralBus::new();
        bus.subscribe(
            0xFF01..=0xFF01,
            Box::new(SpyPeripheral::new(received.clone())),
        );

        mem.write(0xFF01, 0x01).unwrap();
        bus.flush(&mut mem);
        assert_eq!(received.borrow().len(), 1);

        bus.flush(&mut mem); // no new writes — nothing more to dispatch
        assert_eq!(received.borrow().len(), 1);
    }

    #[test]
    fn test_multiple_subscribers_same_range_both_notified() {
        let mut mem = GameBoyMemory::new();
        let received1 = Rc::new(RefCell::new(Vec::new()));
        let received2 = Rc::new(RefCell::new(Vec::new()));
        let mut bus = PeripheralBus::new();
        bus.subscribe(
            0xFF01..=0xFF01,
            Box::new(SpyPeripheral::new(received1.clone())),
        );
        bus.subscribe(
            0xFF01..=0xFF01,
            Box::new(SpyPeripheral::new(received2.clone())),
        );

        mem.write(0xFF01, 0x55).unwrap();
        bus.flush(&mut mem);

        assert_eq!(received1.borrow().len(), 1);
        assert_eq!(received2.borrow().len(), 1);
    }

    #[test]
    fn test_no_subscribers_flush_is_noop() {
        let mut mem = GameBoyMemory::new();
        let mut bus = PeripheralBus::new();
        mem.write(0xFF01, 0x42).unwrap();
        bus.flush(&mut mem); // should not panic
    }

    #[test]
    fn test_event_address_and_value_forwarded_correctly() {
        let mut mem = GameBoyMemory::new();
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut bus = PeripheralBus::new();
        bus.subscribe(
            0xFF00..=0xFF7F,
            Box::new(SpyPeripheral::new(received.clone())),
        );

        mem.write(0xFF42, 0xAB).unwrap();
        bus.flush(&mut mem);

        let events = received.borrow();
        assert_eq!(events[0].address, 0xFF42);
        assert_eq!(events[0].value, 0xAB);
    }
}
