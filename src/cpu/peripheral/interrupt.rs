// Interrupt logic (IE/IF) is now handled directly by the CPU via memory's
// IO register array. See Sm83::has_pending_interrupt, take_pending_interrupt,
// and dispatch_interrupt in sm83.rs.
