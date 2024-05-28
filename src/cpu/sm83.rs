use std::error::Error;

use super::cpu::Cpu;
use super::instructions::decoder::Decoder;
use super::instructions::operand::*;
use super::instructions::adc::opcode::Adc;
use super::instructions::add::opcode::{Add8, Add16, AddSP16};
use super::instructions::instructions::{Error as InstructionError, Instructions};
use super::operations::add::*;
use super::registers::{Flags, Registers};

use crate::memory::rom::{Error as RomError, ReadOnlyMemory};

impl From<RomError> for InstructionError {
    fn from(error: RomError) -> Self {
        InstructionError::Failed(format!("Failed to read ROM: {}", error))
    }
}

pub struct Sm83 {
    rom: Box<dyn ReadOnlyMemory>,
    registers: Registers,
    opcodes: Box<dyn Decoder>,
}

impl Sm83 {
    pub fn new(rom: Box<dyn ReadOnlyMemory>, opcode_decoder: Box<dyn Decoder>) -> Self {
        Self {
            rom,
            registers: Registers::default(),
            opcodes: opcode_decoder,
        }
    }

    // Retrieve a copy of the CPU registers.
    pub fn registers(&self) -> Registers {
        self.registers.clone()
    }

    fn read_next_pc(&mut self) -> Result<u8, RomError> {
        let byte = self.rom.read(self.registers.pc)?;
        self.registers.pc = self.registers.pc.wrapping_add(1);
        Ok(byte)
    }

    fn get_8bit_operand(&mut self, operand: Operand) -> Result<u8, InstructionError> {
        match operand {
            Operand::Register8(reg) => Ok(self.get_register8_operand(reg)),
            Operand::Memory(Memory::HL) => return Err(InstructionError::InvalidOperand(format!("{} not implemented yet.", operand))),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::rom::ROMVec;
    use crate::cpu::registers::Flags;
    use crate::cpu::instructions::opcodes::OpCodeDecoder;

    impl Sm83 {
        pub fn set_registers(mut self, registers: Registers) -> Sm83 {
            self.registers = registers;
            self
        }
    }

    pub fn make_test_cpu(rom_data: Vec<u8>) -> Sm83 {
        let rom: Box<ROMVec> = Box::new(ROMVec::new(rom_data));
        let decoder = Box::new(OpCodeDecoder::new());

        Sm83::new(rom, decoder)
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

    // todo: MMU not implemented yet.
    #[test]
    fn test_add8_memory_to_accumulator() {
        let mut cpu = make_test_cpu(vec![0x86]);
        assert!(cpu.tick().is_err());
    }

    #[test]
    fn test_add8_invalid_opcode() {
        let rom: Box<ROMVec> = Box::new(ROMVec::new(vec![0]));
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(rom, decoder));
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
        let rom: Box<ROMVec> = Box::new(ROMVec::new(vec![0]));
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(rom, decoder));
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
        let rom: Box<ROMVec> = Box::new(ROMVec::new(vec![0]));
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(rom, decoder));
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

    // todo: mmu not implemented yet
    #[test]
    fn test_adc_memhl() {
        let mut cpu = make_test_cpu(vec![0x8E]);
        assert!(cpu.tick().is_err());
    }

    #[test]
    fn test_adc_imm8() {
        let mut cpu = make_test_cpu(vec![0xCE, 0x03]).set_registers(Registers{a: 0x05,..Default::default()});

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding immediate 8-bit value to A
    }

    #[test]
    fn test_adc_invalid_operand() {
        let rom: Box<ROMVec> = Box::new(ROMVec::new(vec![0]));
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(rom, decoder));
        assert!(cpu.adc(&Adc{operand: Operand::Register16(Register16::BC), cycles: 4}).is_err());
    }
}
