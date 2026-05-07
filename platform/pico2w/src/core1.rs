#![cfg(target_arch = "arm")]

extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_rp::multicore::{spawn_core1, Stack};
use embassy_rp::peripherals::CORE1;
use embassy_rp::Peri;
use rustyboy_core::cpu::peripheral::island::{
    LocalPeripheralBackend, PeripheralAdvanceOutput, PeripheralBackend, PeripheralRegs,
    PeripheralShadowState,
};
use rustyboy_core::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use rustyboy_core::cpu::save_state::{PpuState, TimerState};

const AUDIO_BUFFER_CAPACITY: usize = 4096;
const VRAM_SIZE: usize = 0x2000;
const OAM_SIZE: usize = 0xA0;
const ADVANCE_QUEUE_CAPACITY: usize = 64;

static mut CORE1_STACK: Stack<16_384> = Stack::new();
static mut CORE1_WORKER: MaybeUninit<Core1Worker> = MaybeUninit::uninit();

#[repr(u8)]
#[derive(Clone, Copy)]
enum RequestKind {
    Idle = 0,
    Advance = 1,
    ResetDiv = 2,
    ResetLy = 3,
    ReadApuRegister = 4,
    WriteApuRegister = 5,
    ReadWaveRam = 6,
    WriteWaveRam = 7,
    DrainSamples = 8,
    TimerState = 9,
    PpuState = 10,
    LoadState = 11,
    #[cfg(feature = "perf")]
    TakePpuPerf = 12,
    #[cfg(feature = "perf")]
    TakeApuPerf = 13,
}

#[derive(Clone, Copy, Default)]
struct SharedRegs {
    lcdc: u8,
    stat: u8,
    scy: u8,
    scx: u8,
    lyc: u8,
    bgp: u8,
    obp0: u8,
    obp1: u8,
    wy: u8,
    wx: u8,
    tima: u8,
    tma: u8,
    tac: u8,
    ly: u8,
}

impl SharedRegs {
    fn from_peripheral_regs(regs: &PeripheralRegs<'_>) -> Self {
        Self {
            lcdc: regs.lcdc,
            stat: regs.stat,
            scy: regs.scy,
            scx: regs.scx,
            lyc: regs.lyc,
            bgp: regs.bgp,
            obp0: regs.obp0,
            obp1: regs.obp1,
            wy: regs.wy,
            wx: regs.wx,
            tima: regs.tima,
            tma: regs.tma,
            tac: regs.tac,
            ly: regs.ly,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct SharedAdvanceOutput {
    ly: u8,
    stat: u8,
    tima: u8,
    div: u8,
    nr52: u8,
    flags: u8,
}

impl SharedAdvanceOutput {
    const VBLANK_INTERRUPT: u8 = 1 << 0;
    const STAT_INTERRUPT: u8 = 1 << 1;
    const TIMER_INTERRUPT: u8 = 1 << 2;

    const fn idle() -> Self {
        Self {
            ly: 0,
            stat: 0,
            tima: 0,
            div: 0,
            nr52: 0,
            flags: 0,
        }
    }

    fn from_output(output: PeripheralAdvanceOutput) -> Self {
        let mut flags = 0;
        if output.vblank_interrupt {
            flags |= Self::VBLANK_INTERRUPT;
        }
        if output.stat_interrupt {
            flags |= Self::STAT_INTERRUPT;
        }
        if output.timer_interrupt {
            flags |= Self::TIMER_INTERRUPT;
        }
        Self {
            ly: output.ly,
            stat: output.stat,
            tima: output.tima,
            div: output.div,
            nr52: output.nr52,
            flags,
        }
    }

    fn into_output(self) -> PeripheralAdvanceOutput {
        PeripheralAdvanceOutput {
            ly: self.ly,
            stat: self.stat,
            vblank_interrupt: self.flags & Self::VBLANK_INTERRUPT != 0,
            stat_interrupt: self.flags & Self::STAT_INTERRUPT != 0,
            tima: self.tima,
            div: self.div,
            timer_interrupt: self.flags & Self::TIMER_INTERRUPT != 0,
            nr52: self.nr52,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct SharedShadowState {
    ly: u8,
    stat: u8,
    tima: u8,
    div: u8,
    nr52: u8,
    vblank_count: u32,
    stat_interrupt_count: u32,
    timer_interrupt_count: u32,
}

impl SharedShadowState {
    const fn idle() -> Self {
        Self {
            ly: 0,
            stat: 0,
            tima: 0,
            div: 0,
            nr52: 0,
            vblank_count: 0,
            stat_interrupt_count: 0,
            timer_interrupt_count: 0,
        }
    }

    fn into_shadow_state(self, completed_seq: u32) -> PeripheralShadowState {
        PeripheralShadowState {
            completed_seq,
            ly: self.ly,
            stat: self.stat,
            tima: self.tima,
            div: self.div,
            nr52: self.nr52,
            vblank_count: self.vblank_count,
            stat_interrupt_count: self.stat_interrupt_count,
            timer_interrupt_count: self.timer_interrupt_count,
        }
    }
}

#[derive(Clone, Copy)]
struct AdvanceRequest {
    ppu_cycles: u16,
    timer_cycles: u16,
    apu_cycles: u16,
    regs: SharedRegs,
}

impl AdvanceRequest {
    const fn idle() -> Self {
        Self {
            ppu_cycles: 0,
            timer_cycles: 0,
            apu_cycles: 0,
            regs: SharedRegs {
                lcdc: 0,
                stat: 0,
                scy: 0,
                scx: 0,
                lyc: 0,
                bgp: 0,
                obp0: 0,
                obp1: 0,
                wy: 0,
                wx: 0,
                tima: 0,
                tma: 0,
                tac: 0,
                ly: 0,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct MailboxRequest {
    kind: RequestKind,
    addr: u16,
    value: u8,
    ppu_cycles: u16,
    timer_cycles: u16,
    apu_cycles: u16,
    regs: SharedRegs,
    timer_state: TimerState,
    ppu_state: PpuState,
}

impl MailboxRequest {
    const fn idle() -> Self {
        Self {
            kind: RequestKind::Idle,
            addr: 0,
            value: 0,
            ppu_cycles: 0,
            timer_cycles: 0,
            apu_cycles: 0,
            regs: SharedRegs {
                lcdc: 0,
                stat: 0,
                scy: 0,
                scx: 0,
                lyc: 0,
                bgp: 0,
                obp0: 0,
                obp1: 0,
                wy: 0,
                wx: 0,
                tima: 0,
                tma: 0,
                tac: 0,
                ly: 0,
            },
            timer_state: TimerState {
                internal_counter: 0,
            },
            ppu_state: PpuState {
                dot: 0,
                ly: 0,
                mode: rustyboy_core::cpu::peripheral::ppu::PpuMode::OamScan,
                window_line_counter: 0,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct MailboxResponse {
    step: SharedAdvanceOutput,
    value: u8,
    audio_len: usize,
    timer_state: TimerState,
    ppu_state: PpuState,
    #[cfg(feature = "perf")]
    ppu_perf: rustyboy_core::cpu::peripheral::ppu::PpuPerfProfile,
    #[cfg(feature = "perf")]
    apu_perf: rustyboy_core::cpu::peripheral::apu::ApuPerfProfile,
}

impl MailboxResponse {
    const fn idle() -> Self {
        Self {
            step: SharedAdvanceOutput::idle(),
            value: 0,
            audio_len: 0,
            timer_state: TimerState {
                internal_counter: 0,
            },
            ppu_state: PpuState {
                dot: 0,
                ly: 0,
                mode: rustyboy_core::cpu::peripheral::ppu::PpuMode::OamScan,
                window_line_counter: 0,
            },
            #[cfg(feature = "perf")]
            ppu_perf: rustyboy_core::cpu::peripheral::ppu::PpuPerfProfile {
                render_bg: 0,
                render_window: 0,
                render_sprites: 0,
                build_stat: 0,
            },
            #[cfg(feature = "perf")]
            apu_perf: rustyboy_core::cpu::peripheral::apu::ApuPerfProfile {
                frame_seq: 0,
                pulse: 0,
                wave: 0,
                noise: 0,
                mix: 0,
            },
        }
    }
}

struct Mailbox {
    request_seq: AtomicU32,
    response_seq: AtomicU32,
    request: UnsafeCell<MailboxRequest>,
    response: UnsafeCell<MailboxResponse>,
    sync_vram: UnsafeCell<[u8; VRAM_SIZE]>,
    sync_oam: UnsafeCell<[u8; OAM_SIZE]>,
    frame: UnsafeCell<[u8; FRAMEBUFFER_SIZE]>,
    audio: UnsafeCell<[f32; AUDIO_BUFFER_CAPACITY]>,
}

unsafe impl Sync for Mailbox {}

impl Mailbox {
    const fn new() -> Self {
        Self {
            request_seq: AtomicU32::new(0),
            response_seq: AtomicU32::new(0),
            request: UnsafeCell::new(MailboxRequest::idle()),
            response: UnsafeCell::new(MailboxResponse::idle()),
            sync_vram: UnsafeCell::new([0; VRAM_SIZE]),
            sync_oam: UnsafeCell::new([0; OAM_SIZE]),
            frame: UnsafeCell::new([0; FRAMEBUFFER_SIZE]),
            audio: UnsafeCell::new([0.0; AUDIO_BUFFER_CAPACITY]),
        }
    }
}

static MAILBOX: Mailbox = Mailbox::new();

struct AdvanceQueue {
    submit_seq: AtomicU32,
    complete_seq: AtomicU32,
    requests: [UnsafeCell<AdvanceRequest>; ADVANCE_QUEUE_CAPACITY],
    outputs: [UnsafeCell<SharedShadowState>; ADVANCE_QUEUE_CAPACITY],
}

unsafe impl Sync for AdvanceQueue {}

impl AdvanceQueue {
    const fn new() -> Self {
        Self {
            submit_seq: AtomicU32::new(0),
            complete_seq: AtomicU32::new(0),
            requests: [const { UnsafeCell::new(AdvanceRequest::idle()) }; ADVANCE_QUEUE_CAPACITY],
            outputs: [const { UnsafeCell::new(SharedShadowState::idle()) }; ADVANCE_QUEUE_CAPACITY],
        }
    }
}

static ADVANCE_QUEUE: AdvanceQueue = AdvanceQueue::new();

struct Core1Worker {
    backend: LocalPeripheralBackend,
    vblank_count: u32,
    stat_interrupt_count: u32,
    timer_interrupt_count: u32,
}

impl Core1Worker {
    unsafe fn init_in_place(dst: *mut Self) {
        LocalPeripheralBackend::init_in_place(core::ptr::addr_of_mut!((*dst).backend));
        core::ptr::addr_of_mut!((*dst).vblank_count).write(0);
        core::ptr::addr_of_mut!((*dst).stat_interrupt_count).write(0);
        core::ptr::addr_of_mut!((*dst).timer_interrupt_count).write(0);
    }

    fn advance_backend(&mut self, request: AdvanceRequest) -> PeripheralAdvanceOutput {
        let output = self.backend.advance(
            request.ppu_cycles,
            request.timer_cycles,
            request.apu_cycles,
            PeripheralRegs {
                lcdc: request.regs.lcdc,
                stat: request.regs.stat,
                scy: request.regs.scy,
                scx: request.regs.scx,
                lyc: request.regs.lyc,
                bgp: request.regs.bgp,
                obp0: request.regs.obp0,
                obp1: request.regs.obp1,
                wy: request.regs.wy,
                wx: request.regs.wx,
                tima: request.regs.tima,
                tma: request.regs.tma,
                tac: request.regs.tac,
                ly: request.regs.ly,
                vram: unsafe { &*MAILBOX.sync_vram.get() },
                oam: unsafe { &*MAILBOX.sync_oam.get() },
            },
        );
        if output.vblank_interrupt {
            self.backend
                .snapshot_framebuffer_into(unsafe { &mut *MAILBOX.frame.get() });
        }
        output
    }

    fn handle_advance(&mut self, request: AdvanceRequest) -> SharedAdvanceOutput {
        SharedAdvanceOutput::from_output(self.advance_backend(request))
    }

    fn publish_advance_shadow(&mut self, request: AdvanceRequest) -> SharedShadowState {
        let output = self.advance_backend(request);
        if output.vblank_interrupt {
            self.vblank_count = self.vblank_count.wrapping_add(1);
        }
        if output.stat_interrupt {
            self.stat_interrupt_count = self.stat_interrupt_count.wrapping_add(1);
        }
        if output.timer_interrupt {
            self.timer_interrupt_count = self.timer_interrupt_count.wrapping_add(1);
        }
        SharedShadowState {
            ly: output.ly,
            stat: output.stat,
            tima: output.tima,
            div: output.div,
            nr52: output.nr52,
            vblank_count: self.vblank_count,
            stat_interrupt_count: self.stat_interrupt_count,
            timer_interrupt_count: self.timer_interrupt_count,
        }
    }

    fn handle(&mut self, request: MailboxRequest) -> MailboxResponse {
        let mut response = MailboxResponse::idle();
        match request.kind {
            RequestKind::Idle => {}
            RequestKind::Advance => {
                response.step = self.handle_advance(AdvanceRequest {
                    ppu_cycles: request.ppu_cycles,
                    timer_cycles: request.timer_cycles,
                    apu_cycles: request.apu_cycles,
                    regs: request.regs,
                });
            }
            RequestKind::ResetDiv => self.backend.reset_div(),
            RequestKind::ResetLy => self.backend.reset_ly(),
            RequestKind::ReadApuRegister => {
                response.value = self.backend.read_apu_register(request.addr);
            }
            RequestKind::WriteApuRegister => {
                self.backend.write_apu_register(request.addr, request.value);
                response.value = self.backend.read_apu_register(request.addr);
            }
            RequestKind::ReadWaveRam => {
                response.value = self.backend.read_wave_ram(request.value);
            }
            RequestKind::WriteWaveRam => {
                self.backend
                    .write_wave_ram(request.value, request.addr as u8);
                response.value = self.backend.read_wave_ram(request.value);
            }
            RequestKind::DrainSamples => {
                let samples = self.backend.drain_samples();
                let len = samples.len().min(AUDIO_BUFFER_CAPACITY);
                (unsafe { &mut *MAILBOX.audio.get() })[..len].copy_from_slice(&samples[..len]);
                response.audio_len = len;
            }
            RequestKind::TimerState => {
                response.timer_state = self.backend.timer_state();
            }
            RequestKind::PpuState => {
                response.ppu_state = self.backend.ppu_state();
            }
            RequestKind::LoadState => {
                self.backend.load_state(
                    request.timer_state,
                    request.ppu_state,
                    unsafe { &*MAILBOX.sync_vram.get() },
                    unsafe { &*MAILBOX.sync_oam.get() },
                );
            }
            #[cfg(feature = "perf")]
            RequestKind::TakePpuPerf => {
                response.ppu_perf = self.backend.take_ppu_perf_profile();
            }
            #[cfg(feature = "perf")]
            RequestKind::TakeApuPerf => {
                response.apu_perf = self.backend.take_apu_perf_profile();
            }
        }
        response
    }
}

pub struct Core1PeripheralBackend {
    submitted_advance_seq: u32,
}

impl Core1PeripheralBackend {
    fn new() -> Self {
        Self {
            submitted_advance_seq: 0,
        }
    }

    fn transact_shared(request: MailboxRequest) -> MailboxResponse {
        let ticket = MAILBOX.request_seq.load(Ordering::Relaxed).wrapping_add(1);
        unsafe {
            *MAILBOX.request.get() = request;
        }
        MAILBOX.request_seq.store(ticket, Ordering::Release);
        while MAILBOX.response_seq.load(Ordering::Acquire) != ticket {
            spin_loop();
        }
        unsafe { *MAILBOX.response.get() }
    }

    fn transact(&mut self, request: MailboxRequest) -> MailboxResponse {
        Self::transact_shared(request)
    }

    fn copy_sync_memory(vram: &[u8], oam: &[u8]) {
        unsafe {
            (&mut *MAILBOX.sync_vram.get()).copy_from_slice(vram);
            (&mut *MAILBOX.sync_oam.get()).copy_from_slice(oam);
        }
    }

    fn try_queue_advance_request(&mut self, request: AdvanceRequest) -> bool {
        let next_seq = self.submitted_advance_seq.wrapping_add(1);
        let completed = ADVANCE_QUEUE.complete_seq.load(Ordering::Acquire);
        if next_seq.wrapping_sub(completed) > ADVANCE_QUEUE_CAPACITY as u32 {
            return false;
        }

        let index = (next_seq.wrapping_sub(1) as usize) % ADVANCE_QUEUE_CAPACITY;
        unsafe {
            *ADVANCE_QUEUE.requests[index].get() = request;
        }
        ADVANCE_QUEUE.submit_seq.store(next_seq, Ordering::Release);
        self.submitted_advance_seq = next_seq;
        true
    }

    fn try_take_queued_shadow_state(&mut self, min_completed_seq: u32) -> Option<PeripheralShadowState> {
        loop {
            let completed = ADVANCE_QUEUE.complete_seq.load(Ordering::Acquire);
            if completed < min_completed_seq {
                return None;
            }

            let index = (completed.wrapping_sub(1) as usize) % ADVANCE_QUEUE_CAPACITY;
            let shadow = unsafe { *ADVANCE_QUEUE.outputs[index].get() };
            let confirm = ADVANCE_QUEUE.complete_seq.load(Ordering::Acquire);
            if confirm == completed {
                return Some(shadow.into_shadow_state(completed));
            }
        }
    }

    fn wait_queued_shadow_state(&mut self, min_completed_seq: u32) -> PeripheralShadowState {
        loop {
            if let Some(shadow) = self.try_take_queued_shadow_state(min_completed_seq) {
                return shadow;
            }
            spin_loop();
        }
    }
}

impl PeripheralBackend for Core1PeripheralBackend {
    fn advance(
        &mut self,
        ppu_cycles: u16,
        timer_cycles: u16,
        apu_cycles: u16,
        regs: PeripheralRegs<'_>,
    ) -> PeripheralAdvanceOutput {
        let response = self.transact(MailboxRequest {
            kind: RequestKind::Advance,
            addr: 0,
            value: 0,
            ppu_cycles,
            timer_cycles,
            apu_cycles,
            regs: SharedRegs::from_peripheral_regs(&regs),
            timer_state: TimerState {
                internal_counter: 0,
            },
            ppu_state: PpuState {
                dot: 0,
                ly: 0,
                mode: rustyboy_core::cpu::peripheral::ppu::PpuMode::OamScan,
                window_line_counter: 0,
            },
        });
        response.step.into_output()
    }

    fn supports_queued_advance(&self) -> bool {
        true
    }

    fn try_queue_advance(
        &mut self,
        ppu_cycles: u16,
        timer_cycles: u16,
        apu_cycles: u16,
        regs: PeripheralRegs<'_>,
    ) -> bool {
        self.try_queue_advance_request(AdvanceRequest {
            ppu_cycles,
            timer_cycles,
            apu_cycles,
            regs: SharedRegs::from_peripheral_regs(&regs),
        })
    }

    fn try_take_queued_shadow_state(
        &mut self,
        min_completed_seq: u32,
    ) -> Option<PeripheralShadowState> {
        Core1PeripheralBackend::try_take_queued_shadow_state(self, min_completed_seq)
    }

    fn wait_queued_shadow_state(&mut self, min_completed_seq: u32) -> PeripheralShadowState {
        Core1PeripheralBackend::wait_queued_shadow_state(self, min_completed_seq)
    }

    fn reset_div(&mut self) {
        let _ = self.transact(MailboxRequest {
            kind: RequestKind::ResetDiv,
            ..MailboxRequest::idle()
        });
    }

    fn reset_ly(&mut self) {
        let _ = self.transact(MailboxRequest {
            kind: RequestKind::ResetLy,
            ..MailboxRequest::idle()
        });
    }

    fn read_apu_register(&mut self, address: u16) -> u8 {
        self.transact(MailboxRequest {
            kind: RequestKind::ReadApuRegister,
            addr: address,
            ..MailboxRequest::idle()
        })
        .value
    }

    fn write_apu_register(&mut self, address: u16, value: u8) {
        let _ = self.transact(MailboxRequest {
            kind: RequestKind::WriteApuRegister,
            addr: address,
            value,
            ..MailboxRequest::idle()
        });
    }

    fn read_wave_ram(&mut self, offset: u8) -> u8 {
        self.transact(MailboxRequest {
            kind: RequestKind::ReadWaveRam,
            value: offset,
            ..MailboxRequest::idle()
        })
        .value
    }

    fn write_wave_ram(&mut self, offset: u8, value: u8) {
        let _ = self.transact(MailboxRequest {
            kind: RequestKind::WriteWaveRam,
            addr: value as u16,
            value: offset,
            ..MailboxRequest::idle()
        });
    }

    fn on_vram_write(&mut self, offset: u16, value: u8) {
        unsafe {
            (*MAILBOX.sync_vram.get())[offset as usize] = value;
        }
    }

    fn on_oam_write(&mut self, offset: u16, value: u8) {
        unsafe {
            (*MAILBOX.sync_oam.get())[offset as usize] = value;
        }
    }

    fn sync_memory(&mut self, vram: &[u8], oam: &[u8]) {
        Self::copy_sync_memory(vram, oam);
    }

    fn snapshot_framebuffer_into(&mut self, dst: &mut [u8; FRAMEBUFFER_SIZE]) {
        dst.copy_from_slice(unsafe { &*MAILBOX.frame.get() });
    }

    fn drain_samples(&mut self) -> Vec<f32> {
        let response = self.transact(MailboxRequest {
            kind: RequestKind::DrainSamples,
            ..MailboxRequest::idle()
        });
        unsafe { (&*MAILBOX.audio.get())[..response.audio_len].to_vec() }
    }

    fn clear_samples(&mut self) {
        let _ = self.transact(MailboxRequest {
            kind: RequestKind::DrainSamples,
            ..MailboxRequest::idle()
        });
    }

    fn timer_state(&self) -> TimerState {
        Self::transact_shared(MailboxRequest {
            kind: RequestKind::TimerState,
            ..MailboxRequest::idle()
        })
        .timer_state
    }

    fn ppu_state(&self) -> PpuState {
        Self::transact_shared(MailboxRequest {
            kind: RequestKind::PpuState,
            ..MailboxRequest::idle()
        })
        .ppu_state
    }

    fn load_state(&mut self, timer: TimerState, ppu: PpuState, vram: &[u8], oam: &[u8]) {
        Self::copy_sync_memory(vram, oam);
        let _ = self.transact(MailboxRequest {
            kind: RequestKind::LoadState,
            timer_state: timer,
            ppu_state: ppu,
            ..MailboxRequest::idle()
        });
    }

    #[cfg(feature = "perf")]
    fn take_ppu_perf_profile(&mut self) -> rustyboy_core::cpu::peripheral::ppu::PpuPerfProfile {
        self.transact(MailboxRequest {
            kind: RequestKind::TakePpuPerf,
            ..MailboxRequest::idle()
        })
        .ppu_perf
    }

    #[cfg(feature = "perf")]
    fn take_apu_perf_profile(&mut self) -> rustyboy_core::cpu::peripheral::apu::ApuPerfProfile {
        self.transact(MailboxRequest {
            kind: RequestKind::TakeApuPerf,
            ..MailboxRequest::idle()
        })
        .apu_perf
    }
}

fn core1_main() -> ! {
    let worker = core::ptr::addr_of_mut!(CORE1_WORKER).cast::<Core1Worker>();
    unsafe {
        Core1Worker::init_in_place(worker);
    }
    let mut last_ticket = 0u32;
    let mut completed_advance_seq = 0u32;
    loop {
        let submitted_advance_seq = ADVANCE_QUEUE.submit_seq.load(Ordering::Acquire);
        if completed_advance_seq != submitted_advance_seq {
            let next_seq = completed_advance_seq.wrapping_add(1);
            let index = (next_seq.wrapping_sub(1) as usize) % ADVANCE_QUEUE_CAPACITY;
            let request = unsafe { *ADVANCE_QUEUE.requests[index].get() };
            let shadow = unsafe { (&mut *worker).publish_advance_shadow(request) };
            unsafe {
                *ADVANCE_QUEUE.outputs[index].get() = shadow;
            }
            completed_advance_seq = next_seq;
            ADVANCE_QUEUE
                .complete_seq
                .store(completed_advance_seq, Ordering::Release);
            continue;
        }

        let ticket = MAILBOX.request_seq.load(Ordering::Acquire);
        if ticket == last_ticket {
            spin_loop();
            continue;
        }
        let request = unsafe { *MAILBOX.request.get() };
        let response = unsafe { (&mut *worker).handle(request) };
        unsafe {
            *MAILBOX.response.get() = response;
        }
        MAILBOX.response_seq.store(ticket, Ordering::Release);
        last_ticket = ticket;
    }
}

pub fn spawn_peripheral_core(core1: Peri<'static, CORE1>) -> Box<dyn PeripheralBackend> {
    spawn_core1(
        core1,
        unsafe { &mut *core::ptr::addr_of_mut!(CORE1_STACK) },
        move || core1_main(),
    );

    Box::new(Core1PeripheralBackend::new())
}
