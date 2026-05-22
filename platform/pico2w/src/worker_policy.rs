use rustyboy_core::ipc::WorkerCommand;

#[inline(always)]
pub(crate) fn should_drain_audio_after_worker_command(_command: &WorkerCommand) -> bool {
    // Audio samples cross cores through an explicit DrainAudio barrier so the
    // producer and consumer never touch the queue concurrently.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_commands_do_not_drain_audio_directly() {
        let commands = [
            WorkerCommand::AdvanceApu {
                cycles: 1024,
                div_counter: 0x2000,
            },
            WorkerCommand::AdvancePpu { cycles: 24 },
            WorkerCommand::WriteApuRegister {
                addr: 0xFF12,
                value: 0xF3,
            },
            WorkerCommand::WriteWaveRam {
                offset: 0x02,
                value: 0xAA,
            },
            WorkerCommand::WritePpuRegister {
                addr: 0xFF40,
                value: 0x91,
            },
            WorkerCommand::WriteVram {
                offset: 0x1800,
                value: 0x2A,
            },
            WorkerCommand::WriteOam {
                offset: 0x20,
                value: 0x80,
            },
        ];

        for command in commands {
            assert!(
                !should_drain_audio_after_worker_command(&command),
                "worker command {command:?} must not drain the audio queue directly; SML can crash when core1 enqueues while core0 drains"
            );
        }
    }
}
