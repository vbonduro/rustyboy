use std::error::Error;

use super::cpu::Cpu;
use super::instructions::decoder::Decoder;
use super::instructions::operand::*;
use super::instructions::adc::opcode::Adc;
use super::instructions::add::opcode::{Add8, Add16, AddSP16};
use super::instructions::sub::opcode::Sub8;
use super::instructions::sbc::opcode::Sbc8;
use super::instructions::cp::opcode::Cp8;
use super::instructions::ld::opcode::Ld8;
use super::instructions::instructions::{Error as InstructionError, Instructions};
use super::operations::add::*;
use super::operations::sub::*;
use super::registers::{Flags, Registers};

use crate::memory::memory::{Error as MemoryError, Memory as MemoryBus};

impl From<MemoryError> for InstructionError {
    fn from(error: MemoryError) -> Self {
        InstructionError::Failed(format!("Failed to access memory: {}", error))
    }
}

pub struct Sm83 {
    memory: Box<dyn MemoryBus>,
    registers: Registers,
    opcodes: Box<dyn Decoder>,
}

impl Sm83 {
    pub fn new(memory: Box<dyn MemoryBus>, opcode_decoder: Box<dyn Decoder>) -> Self {
        Self {
            memory,
            registers: Registers::default(),
            opcodes: opcode_decoder,
        }
    }

    // Retrieve a copy of the CPU registers.
    pub fn registers(&self) -> Registers {
        self.registers.clone()
    }

    fn read_next_pc(&mut self) -> Result<u8, MemoryError> {
        let byte = self.memory.read(self.registers.pc)?;
        self.registers.pc = self.registers.pc.wrapping_add(1);
        Ok(byte)
    }

    fn get_8bit_operand(&mut self, operand: Operand) -> Result<u8, InstructionError> {
        match operand {
            Operand::Register8(reg) => Ok(self.get_register8_operand(reg)),
            Operand::Memory(Memory::HL) => {
                let address = self.registers.hl();
                Ok(self.memory.read(address)?)
            },
            Operand::Imm8 => Ok(self.read_next_pc()?),
            _ => return Err(InstructionError::InvalidOperand(format!("{} for instruction Add8", operand))),
        }
    }

    fn get_register8_operand(&self, operand: Register8) -> u8 {
        match operand {
            Register8::A => self.registers.a,
            Register8::B => self.registers.b,
            Register8::C => self.registers.c,
            Register8::D => self.registers.d,
            Register8::E => self.registers.e,
            Register8::H => self.registers.h,
            Register8::L => self.registers.l,
        }
    }

    fn get_register16_operand(&self, operand: Register16) -> u16 {
        match operand {
            Register16::BC => self.registers.bc(),
            Register16::DE => self.registers.de(),
            Register16::HL => self.registers.hl(),
            Register16::SP => self.registers.sp,
        }
    }

    fn set_register8_operand(&mut self, operand: Register8, value: u8) {
        match operand {
            Register8::A => self.registers.a = value,
            Register8::B => self.registers.b = value,
            Register8::C => self.registers.c = value,
            Register8::D => self.registers.d = value,
            Register8::E => self.registers.e = value,
            Register8::H => self.registers.h = value,
            Register8::L => self.registers.l = value,
        }
    }
}

impl Cpu for Sm83 {
     fn tick(&mut self) -> Result<u8, Box<dyn Error>> {
        let opcode = self.read_next_pc()?;

        Ok(self.opcodes.decode(opcode)?.execute(self)?)
    }
}

impl Instructions for Sm83 {
    fn add8(&mut self, opcode: &Add8) -> Result<u8, InstructionError> {
        (self.registers.a, self.registers.f) = add_u8(self.registers.a, self.get_8bit_operand(opcode.operand)?);
        Ok(opcode.cycles)
    }

    fn add16(&mut self, opcode: &Add16) -> Result<u8, InstructionError> {
        let operand: u16 = match opcode.operand {
            Operand::Register16(reg) => self.get_register16_operand(reg),
            _ => return Err(InstructionError::InvalidOperand(format!("{} for instruction Add16", opcode.operand))),
        };

        let hl: u16;
        (hl, self.registers.f) = add_u16(self.registers.hl(), operand);
        self.registers.set_hl(hl);
        Ok(opcode.cycles)
    }

    fn add_sp16(&mut self, opcode: &AddSP16) -> Result<u8, InstructionError> {
        if opcode.operand != Operand::ImmSigned8 {
            return Err(InstructionError::InvalidOperand(format!("{} for instruction AddSP16", opcode.operand)));
        }

        let operand: u16 = self.read_next_pc()? as i8 as i16 as u16;
        (self.registers.sp, self.registers.f) = add_u16(self.registers.sp, operand);

        Ok(opcode.cycles)
    }

    fn adc(&mut self, opcode: &Adc) -> Result<u8, InstructionError> {
        let carry: u8 = self.registers.f.contains(Flags::C) as u8;

        let flags: Flags;
        (self.registers.a, flags) = add_u8(self.registers.a, carry);
        (self.registers.a, self.registers.f) = add_u8(self.registers.a, self.get_8bit_operand(opcode.operand)?);
        self.registers.f |= flags;

        Ok(opcode.cycles)
    }

    fn sub8(&mut self, opcode: &Sub8) -> Result<u8, InstructionError> {
        (self.registers.a, self.registers.f) = sub_u8(self.registers.a, self.get_8bit_operand(opcode.operand)?);
        Ok(opcode.cycles)
    }

    fn sbc8(&mut self, opcode: &Sbc8) -> Result<u8, InstructionError> {
        let carry: u8 = self.registers.f.contains(Flags::C) as u8;
        (self.registers.a, self.registers.f) = sbc_u8(self.registers.a, self.get_8bit_operand(opcode.operand)?, carry);
        Ok(opcode.cycles)
    }

    fn cp8(&mut self, opcode: &Cp8) -> Result<u8, InstructionError> {
        self.registers.f = cp_u8(self.registers.a, self.get_8bit_operand(opcode.operand)?);
        Ok(opcode.cycles)
    }

    fn ld8(&mut self, opcode: &Ld8) -> Result<u8, InstructionError> {
        // Read the source value
        let value = match opcode.src {
            Operand::Register8(reg) => self.get_register8_operand(reg),
            Operand::Memory(Memory::HL) => {
                let address = self.registers.hl();
                self.memory.read(address)?
            },
            Operand::Imm8 => self.read_next_pc()?,
            _ => return Err(InstructionError::InvalidOperand(format!("{} for instruction Ld8 src", opcode.src))),
        };

        // Write to the destination
        match opcode.dest {
            Operand::Register8(reg) => self.set_register8_operand(reg, value),
            Operand::Memory(Memory::HL) => {
                let address = self.registers.hl();
                self.memory.write(address, value)?;
            },
            _ => return Err(InstructionError::InvalidOperand(format!("{} for instruction Ld8 dest", opcode.dest))),
        }

        Ok(opcode.cycles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::memory::GameBoyMemory;
    use crate::memory::fake::FakeMemory;
    use crate::cpu::registers::Flags;
    use crate::cpu::instructions::opcodes::OpCodeDecoder;

    impl Sm83 {
        pub fn set_registers(mut self, registers: Registers) -> Sm83 {
            self.registers = registers;
            self
        }
    }

    pub fn make_test_cpu(rom_data: Vec<u8>) -> Sm83 {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::with_rom(rom_data));
        let decoder = Box::new(OpCodeDecoder::new());

        Sm83::new(memory, decoder)
    }

    pub fn make_test_cpu_with_memory(memory: FakeMemory, rom_data: Vec<u8>) -> Sm83 {
        // Load ROM bytes into FakeMemory starting at address 0
        let mut mem = memory;
        for (i, byte) in rom_data.iter().enumerate() {
            mem.set(i as u16, *byte);
        }
        let decoder = Box::new(OpCodeDecoder::new());
        Sm83::new(Box::new(mem), decoder)
    }

    /// Add a constant to the accumulator register and expect the register's value to be the
    /// appropriate value.
    #[test]
    fn test_add8_imm8_to_accumlator() {
        let mut cpu = make_test_cpu(vec![0xC6, 0x03]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x03);
        assert_eq!(cpu.registers().f, Flags::empty())
    }

    #[test]
    fn test_add8_imm8_to_accumlator_sum_zero() {
        let mut cpu = make_test_cpu(vec![0xC6, 0x00]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z)
    }

    // Set the expected value for register b and confirm the add operation takes place as expected.
    #[test]
    fn test_add8_regb_to_accumulator() {
        let mut cpu = make_test_cpu(vec![0x80]).set_registers(Registers{b: 0x05, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x05);
    }

    /// ADD A, (HL) — opcode 0x86 — reads from memory at address pointed to by HL.
    /// HL=0xC000, memory[0xC000]=0x07, A=0x03 → A should become 0x0A.
    #[test]
    fn test_add8_memory_hl_to_accumulator() {
        let mut mem = FakeMemory::new();
        mem.set(0xC000, 0x07); // value at (HL)
        let mut cpu = make_test_cpu_with_memory(mem, vec![0x86])
            .set_registers(Registers{a: 0x03, h: 0xC0, l: 0x00, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0A);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_add8_invalid_opcode() {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::new());
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(memory, decoder));
        assert!(cpu.add8(&Add8{operand: Operand::Imm16, cycles: 4}).is_err());
    }

    // Load up all 8-bit registers with some test values, add them all to the accumulator register, the add the accumulator
    // register to itself, then confirm that it has the expected value.
    #[test]
    fn test_add8_all_reg8s_to_accumulator() {
        let registers = Registers{b: 0x01, c: 0x02, d: 0x03, e: 0x04, h: 0x05, l: 0x06, ..Default::default()};
        let rom_data = vec![0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x87];
        let num_instructions = rom_data.len();
        let mut cpu = make_test_cpu(rom_data).
            set_registers(registers.clone());
        
        let total_cycles: u8 = (0..num_instructions).map(|_| cpu.tick().unwrap()).sum();

        let mut expected_accumlator_value = registers.b + registers.c + registers.d + registers.e + registers.h + registers.l;
        expected_accumlator_value += expected_accumlator_value;

        assert_eq!(total_cycles, num_instructions as u8*4);
        assert_eq!(cpu.registers().a, expected_accumlator_value);
    }

    #[test]
    fn test_add8_rollover_flags() {
        let mut cpu = make_test_cpu(vec![0xC6, 0xFF]).set_registers(Registers{a: 0x01, ..Default::default()}); // Add 0xFF to accumulator
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00); // Accumulator should have rolled over to 0.
        assert_eq!(cpu.registers().f, Flags::Z | Flags::C | Flags::H); // Flags should indicate zero, carry, and half-carry
    }

    #[test]
    fn test_add16_bc_to_hl() {
        let mut registers = Registers::default();
        registers.set_bc(0xbeef);
        let mut cpu = make_test_cpu(vec![0x09]).set_registers(registers);
    
        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().hl(), 0xbeef); // Expected value after adding BC to HL
    }

    #[test]
    fn test_add16_de_to_hl() {
        let mut registers = Registers::default();
        registers.set_de(0xbeef);
        let mut cpu = make_test_cpu(vec![0x19]).set_registers(registers);

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().hl(), 0xbeef); // Expected value after adding DE to HL
    }

    #[test]
    fn test_add16_hl_to_hl() {
        let mut registers = Registers::default();
        registers.set_hl(0xffff);
        let mut cpu = make_test_cpu(vec![0x29]).set_registers(registers);

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().hl(), 0xfffe); // Expected value after adding HL to HL
        assert_eq!(cpu.registers().f, Flags::H | Flags::C);
    }

    #[test]
    fn test_add16_sp_to_hl() {
        let mut registers = Registers::default();
        registers.sp = 0xffff;
        let mut cpu = make_test_cpu(vec![0x39]).set_registers(registers);

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().hl(), 0xffff); // Expected value after adding SP to HL
    }

    #[test]
    fn test_add16_invalid_opcode() {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::new());
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(memory, decoder));
        assert!(cpu.add16(&Add16{operand: Operand::Imm8, cycles: 4}).is_err());
    }

    // TODO: This is NOT a useful test. Will have to revisit later.
    #[test]
    fn test_add_sp16_imm8() {
        let mut cpu = make_test_cpu(vec![0xE8, 0x05]);

        assert_eq!(cpu.tick().unwrap(), 16);
        assert_eq!(cpu.registers().sp, 0x0005); // Expected value after adding signed immediate 8-bit value to SP
    }

    #[test]
    fn test_add_sp16_invalid_opcode() {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::new());
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(memory, decoder));
        assert!(cpu.add_sp16(&AddSP16{operand: Operand::Imm8, cycles: 4}).is_err());
    }

    #[test]
    fn test_adc_b() {
        let mut cpu = make_test_cpu(vec![0x88]).set_registers(Registers{a: 0x05, b: 0x03, ..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding B to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_c() {
        let mut cpu = make_test_cpu(vec![0x89]).set_registers(Registers{a: 0x05, c: 0x03, ..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding C to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_d() {
        let mut cpu = make_test_cpu(vec![0x8A]).set_registers(Registers{a: 0x05, d: 0x03, ..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding D to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_e() {
        let mut cpu = make_test_cpu(vec![0x8B]).set_registers(Registers{a: 0x05, e: 0x03, ..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding E to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_h() {
        let mut cpu = make_test_cpu(vec![0x8C]).set_registers(Registers{a: 0x05, h: 0x03,..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding H to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_l() {
        let mut cpu = make_test_cpu(vec![0x8D]).set_registers(Registers{a: 0x05, l: 0x03,..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding L to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_a() {
        let mut cpu = make_test_cpu(vec![0x8F]).set_registers(Registers{a: 0x05,..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x0A); // Expected value after adding A to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    /// ADC A, (HL) — opcode 0x8E — reads from memory at address in HL, adds with carry.
    /// HL=0xC001, memory[0xC001]=0x04, A=0x05, carry=0 → A should become 0x09.
    #[test]
    fn test_adc_memhl() {
        let mut mem = FakeMemory::new();
        mem.set(0xC001, 0x04);
        let mut cpu = make_test_cpu_with_memory(mem, vec![0x8E])
            .set_registers(Registers{a: 0x05, h: 0xC0, l: 0x01, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x09);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_imm8() {
        let mut cpu = make_test_cpu(vec![0xCE, 0x03]).set_registers(Registers{a: 0x05,..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding immediate 8-bit value to A
    }

    #[test]
    fn test_adc_invalid_operand() {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::new());
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(memory, decoder));
        assert!(cpu.adc(&Adc{operand: Operand::Register16(Register16::BC), cycles: 4}).is_err());
    }

    #[test]
    fn test_sub8_imm8() {
        let mut cpu = make_test_cpu(vec![0xD6, 0x03]).set_registers(Registers{a: 0x05, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x02);
        assert_eq!(cpu.registers().f, Flags::N);
    }

    #[test]
    fn test_sub8_zero_result() {
        let mut cpu = make_test_cpu(vec![0xD6, 0x05]).set_registers(Registers{a: 0x05, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::N);
    }

    #[test]
    fn test_sub8_regb() {
        let mut cpu = make_test_cpu(vec![0x90]).set_registers(Registers{a: 0x10, b: 0x05, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x0B);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H);  // H flag should be set for 0x10 - 0x05
    }

    #[test]
    fn test_sub8_borrow() {
        let mut cpu = make_test_cpu(vec![0xD6, 0x10]).set_registers(Registers{a: 0x05, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0xF5);
        assert_eq!(cpu.registers().f, Flags::N | Flags::C);
    }

    #[test]
    fn test_sub8_half_borrow() {
        let mut cpu = make_test_cpu(vec![0xD6, 0x01]).set_registers(Registers{a: 0x10, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0F);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H);
    }

    #[test]
    fn test_sbc8_no_carry() {
        let mut cpu = make_test_cpu(vec![0xDE, 0x03]).set_registers(Registers{a: 0x10, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0D);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H);  // Half-borrow: 0x0 < 0x3
    }

    #[test]
    fn test_sbc8_with_carry() {
        let mut cpu = make_test_cpu(vec![0xDE, 0x03]).set_registers(Registers{a: 0x10, f: Flags::C, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0C);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H);  // Half-borrow: 0x0 < 0x3 + 1
    }

    #[test]
    fn test_sbc8_zero_result() {
        let mut cpu = make_test_cpu(vec![0xDE, 0x04]).set_registers(Registers{a: 0x05, f: Flags::C, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::N);
    }

    #[test]
    fn test_sbc8_regb() {
        let mut cpu = make_test_cpu(vec![0x98]).set_registers(Registers{a: 0x10, b: 0x05, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x0B);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H);
    }

    #[test]
    fn test_cp8_imm8() {
        let mut cpu = make_test_cpu(vec![0xFE, 0x05]).set_registers(Registers{a: 0x05, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x05);  // A register unchanged
        assert_eq!(cpu.registers().f, Flags::Z | Flags::N);
    }

    #[test]
    fn test_cp8_regb() {
        let mut cpu = make_test_cpu(vec![0xB8]).set_registers(Registers{a: 0x05, b: 0x10, ..Default::default()});
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x05);  // A register unchanged
        assert_eq!(cpu.registers().f, Flags::N | Flags::C);
    }

    // --- LD 8-bit integration tests ---

    /// LD B, C — register to register: copies C into B.
    /// C=0x42, opcode 0x41 (LD B,C), expect B=0x42, 4 cycles.
    #[test]
    fn test_ld8_reg_to_reg() {
        let mut cpu = make_test_cpu(vec![0x41])
            .set_registers(Registers { c: 0x42, ..Default::default() });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().b, 0x42);
    }

    /// LD A, (HL) — load from memory at HL into A.
    /// HL=0xC000, memory[0xC000]=0x55, opcode 0x7E, expect A=0x55, 8 cycles.
    #[test]
    fn test_ld8_a_from_mem_hl() {
        let mut mem = FakeMemory::new();
        mem.set(0xC000, 0x55);
        let mut cpu = make_test_cpu_with_memory(mem, vec![0x7E])
            .set_registers(Registers { h: 0xC0, l: 0x00, ..Default::default() });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x55);
    }

    /// LD (HL), A — store A into memory at HL, then LD A, (HL) to verify write.
    /// ROM: [0x77 (LD (HL),A), 0x7E (LD A,(HL))].
    /// A=0xCD, HL=0xC000 initially. After store A=0 is zeroed then restored via the read-back.
    #[test]
    fn test_ld8_mem_hl_from_a_verify_memory() {
        // ROM: store A to (HL), then load A from (HL); HL=0xC000, A=0xCD
        // After tick 1 (LD (HL),A): memory[0xC000]=0xCD, cycles=8
        // After tick 2 (LD A,(HL)): A=0xCD, cycles=8
        let mut memory = GameBoyMemory::with_rom(vec![0x77, 0x7E]);
        let decoder = Box::new(OpCodeDecoder::new());
        let mut cpu = Sm83::new(Box::new(memory), decoder)
            .set_registers(Registers { a: 0xCD, h: 0xC0, l: 0x00, ..Default::default() });

        // Store A into (HL)
        let cycles1 = cpu.tick().unwrap();
        assert_eq!(cycles1, 8);

        // Zero out A (keep HL pointing to 0xC000) so we know the next tick loads from memory
        let regs_after_store = cpu.registers();
        cpu = cpu.set_registers(Registers { a: 0x00, ..regs_after_store });

        // Load A from (HL) — should restore 0xCD from memory
        let cycles2 = cpu.tick().unwrap();
        assert_eq!(cycles2, 8);
        assert_eq!(cpu.registers().a, 0xCD);
    }

    /// LD B, n — load immediate byte into B.
    /// ROM: [0x06, 0x7F], expect B=0x7F, 8 cycles.
    #[test]
    fn test_ld8_b_imm8() {
        let mut cpu = make_test_cpu(vec![0x06, 0x7F]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().b, 0x7F);
    }

    /// LD (HL), n — store immediate byte into memory at HL, then LD A,(HL) to verify.
    /// ROM: [0x36, 0x99, 0x7E], HL=0xC000.
    /// Tick 1: store 0x99 to (HL), 12 cycles.
    /// Tick 2: load A from (HL), expect A=0x99, 8 cycles.
    #[test]
    fn test_ld8_mem_hl_imm8() {
        let mut memory = GameBoyMemory::with_rom(vec![0x36, 0x99, 0x7E]);
        let decoder = Box::new(OpCodeDecoder::new());
        let mut cpu = Sm83::new(Box::new(memory), decoder)
            .set_registers(Registers { h: 0xC0, l: 0x00, ..Default::default() });

        // LD (HL), 0x99
        let cycles1 = cpu.tick().unwrap();
        assert_eq!(cycles1, 12);

        // LD A, (HL) — verify that 0x99 was stored
        let regs = cpu.registers();
        cpu = cpu.set_registers(Registers { a: 0x00, ..regs });
        let cycles2 = cpu.tick().unwrap();
        assert_eq!(cycles2, 8);
        assert_eq!(cpu.registers().a, 0x99);
    }

    /// Integration test: ADD A, (HL) via GameBoyMemory.
    /// ROM: [0x86] at address 0x0000.
    /// Memory at 0xC010 = 0x0F. A = 0x01, HL = 0xC010.
    /// Expected: A = 0x10, H flag set (lower nibble overflow 1+F=10).
    #[test]
    fn test_integration_add8_memory_hl_gameboy_memory() {
        let mut memory = GameBoyMemory::with_rom(vec![0x86]);
        // Pre-populate the memory location HL will point to
        memory.write(0xC010, 0x0F).unwrap();

        let decoder = Box::new(OpCodeDecoder::new());
        let mut cpu = Sm83::new(Box::new(memory), decoder)
            .set_registers(Registers{a: 0x01, h: 0xC0, l: 0x10, ..Default::default()});

        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x10);
        assert_eq!(cpu.registers().f, Flags::H); // half-carry: low nibble 1 + F = 10
    }
}
