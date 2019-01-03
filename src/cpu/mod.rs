mod decode;

use self::decode::DecodeInstruction;
use self::decode::DecodedArmInstruction;
use scheduler::GeneratorTask;
use scheduler::Task;
use system::AccessWidth;
use system::Bus;
use system::MemoryRequest;
use system::OperationType;

// Named constants for common registers
const LR: usize = 14;
const PC: usize = 15;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
struct Cpsr(u32);

macro_rules! flag_field {
    ($getter:ident, $setter:ident, $bit:expr) => {
        fn $getter(&self) -> bool {
            self.0 & (1 << $bit) != 0
        }

        fn $setter(&mut self, c: bool) {
            if c {
                self.0 |= (1 << $bit);
            } else {
                self.0 &= !(1 << $bit);
            }
        }
    };
}

impl Cpsr {
    flag_field!(negative, set_negative, 31);
    flag_field!(zero, set_zero, 30);
    flag_field!(carry, set_carry, 29);
    flag_field!(overflow, set_overflow, 29);
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum ExecuteState {
    PipelineRefill1,
    PipelineRefill2,
    FirstCycle, // for single-cycle instructions, this is the only cycle
}

struct ArmCpu {
    regs: [u32; 16],
    cpsr: Cpsr,
    current_execute_state: ExecuteState,

    // Fetch stage output
    f_out_instr: u32,
    // Decode stage output
    d_out_instr: u32,
}

fn decode_immediate(imm: u8, rotate: u8, carry_in: bool) -> (u32, bool) {
    let result = (imm as u32).rotate_right(rotate as u32 * 2);
    let carry_out = if rotate == 0 {
        carry_in
    } else {
        bit!(result[31]) != 0
    };
    (result, carry_out)
}

fn add_has_signed_overflow(x: u32, y: u32, r: u32) -> bool {
    // Signed overflow happens when the carry into the MSB differs from the carry out of it. This
    // can be detected by comparing the output bit to both inputs. If they're both different, then
    // that means that the MSB was affected by a carry that rippled *into* or *out of* it, as
    // opposed to rippling *through* it, because that's the only scenario where a carry could affect
    // the visible result of that bit such that it's different from the inputs.
    ((x ^ r) & (y ^ r)) & (1 << 31) != 0
}

fn add_with_carry(op1: u32, op2: u32, carry_in: bool) -> (u32, bool, bool) {
    let result = op1 as u64 + op2 as u64 + carry_in as u64;
    let carry = result & (1 << 32) != 0;
    (
        result as u32,
        carry,
        add_has_signed_overflow(op1, op2, result as u32),
    )
}

fn alu_operation(
    opcode: u8,
    op1: u32,
    op2: u32,
    shifter_carry: bool,
    mut cpsr: Cpsr,
) -> (u32, Cpsr) {
    let result = match opcode {
        // AND, TST
        0 | 8 => {
            cpsr.set_carry(shifter_carry);
            op1 & op2
        }
        // EOR, TEQ
        1 | 9 => {
            cpsr.set_carry(shifter_carry);
            op1 ^ op2
        }
        // SUB, CMP
        2 | 10 => {
            let (x, not_c) = op1.overflowing_sub(op2);
            cpsr.set_carry(!not_c);
            cpsr.set_overflow(add_has_signed_overflow(op1, op2, x));

            let (x2, c2, v2) = add_with_carry(op1, !op2, true);
            assert_eq!(x, x2);
            assert_eq!(cpsr.carry(), c2);
            assert_eq!(cpsr.overflow(), v2);

            x
        }
        // RSB
        3 => {
            let (x, not_c) = op2.overflowing_sub(op1);
            cpsr.set_carry(!not_c);
            cpsr.set_overflow(add_has_signed_overflow(op2, op1, x));

            let (x2, c2, v2) = add_with_carry(op2, !op1, true);
            assert_eq!(x, x2);
            assert_eq!(cpsr.carry(), c2);
            assert_eq!(cpsr.overflow(), v2);

            x
        }
        // ADD, CMN
        4 | 11 => {
            let (x, c) = op1.overflowing_add(op2);
            cpsr.set_carry(c);
            cpsr.set_overflow(add_has_signed_overflow(op1, op2, x));
            x
        }
        // ADC
        5 => {
            let (x, c, v) = add_with_carry(op1, op2, cpsr.carry());
            cpsr.set_carry(c);
            cpsr.set_overflow(v);
            x
        }
        // SBC
        6 => {
            let (x, c, v) = add_with_carry(op1, !op2, cpsr.carry());
            cpsr.set_carry(c);
            cpsr.set_overflow(v);
            x
        }
        // RSC
        7 => {
            let (x, c, v) = add_with_carry(op2, !op1, cpsr.carry());
            cpsr.set_carry(c);
            cpsr.set_overflow(v);
            x
        }
        // ORR
        12 => {
            cpsr.set_carry(shifter_carry);
            op1 | op2
        }
        // MOV
        13 => {
            cpsr.set_carry(shifter_carry);
            op2
        }
        // BIC
        14 => {
            cpsr.set_carry(shifter_carry);
            op1 & !op2
        }
        // MVN
        15 => {
            cpsr.set_carry(shifter_carry);
            !op2
        }
        _ => unreachable!(),
    };

    cpsr.set_negative(bit!(result[31]) != 0);
    cpsr.set_zero(result == 0);

    (result, cpsr)
}

impl ArmCpu {
    fn new() -> ArmCpu {
        ArmCpu {
            regs: [0; 16],
            cpsr: Cpsr(0),
            current_execute_state: ExecuteState::PipelineRefill1,

            f_out_instr: 0xFFFFFFFF,
            d_out_instr: 0xFFFFFFFF,
        }
    }

    fn step(&mut self, bus: &Bus) {
        if bus.should_cpu_wait() {
            return;
        }

        self.step_fetch_or_single_instruction(bus);
    }

    fn step_execute_fsm(
        &mut self,
        bus: &Bus,
        current_state: ExecuteState,
        in_instr: u32,
    ) -> ExecuteState {
        match current_state {
            ExecuteState::PipelineRefill1 => {
                self.regs[PC] = self.regs[PC].wrapping_add(4);
                ExecuteState::PipelineRefill2
            }
            ExecuteState::PipelineRefill2 => {
                self.regs[PC] = self.regs[PC].wrapping_add(4);
                ExecuteState::FirstCycle
            }
            ExecuteState::FirstCycle => {
                println!("Executing {:08X}", in_instr);
                // TODO: Handle condition
                let decoded_instr = DecodedArmInstruction::decode_arm_instruction(in_instr);
                match decoded_instr {
                    DecodedArmInstruction::DataProcessingImmediate {
                        cond,
                        opcode,
                        s,
                        rn,
                        rd,
                        rotate,
                        imm,
                    } => {
                        let (imm_value, imm_carry) =
                            decode_immediate(imm, rotate, self.cpsr.carry());
                        let (result, new_cpsr) = alu_operation(
                            opcode,
                            self.regs[rn as usize],
                            imm_value,
                            imm_carry,
                            self.cpsr,
                        );

                        if rd as usize == PC {
                            if s {
                                unimplemented!("Handle restoring SPSR"); // TODO
                            }
                            unimplemented!("Handle PC writes"); // TODO
                        } else {
                            if s {
                                unimplemented!("Handle flags update"); // TODO
                            }

                            match opcode {
                                // TST, TEQ, CMP, CMN
                                8 | 9 | 10 | 11 => (),
                                _ => self.regs[rd as usize] = result,
                            }
                        }
                    }
                    DecodedArmInstruction::BranchImm { cond, link, offset } => {
                        if link {
                            self.regs[LR] = self.regs[PC].wrapping_sub(4);
                        }
                        // TODO: Handle faulting on bad address
                        self.regs[PC] = self.regs[PC].wrapping_add((offset * 4) as u32);
                        println!("Branching to PC={:0X}", self.regs[PC]);
                        return ExecuteState::PipelineRefill1;
                    }
                    instr => unimplemented!("Unimplemented instruction execute: {:?}", instr),
                }

                self.regs[PC].wrapping_add(4);
                return ExecuteState::FirstCycle;
            }
        }
    }

    fn bus_operation_for_state(&self, state: ExecuteState) -> Option<MemoryRequest> {
        match state {
            ExecuteState::PipelineRefill1
            | ExecuteState::PipelineRefill2
            | ExecuteState::FirstCycle => Some(MemoryRequest {
                address: self.regs[PC],
                width: AccessWidth::Bit32,
                op: OperationType::Read {
                    is_instruction: true,
                },
                seq: state != ExecuteState::PipelineRefill1,
            }),
        }
    }

    fn step_fetch_or_single_instruction(&mut self, bus: &Bus) {
        // Pre-read
        let d_in_instr = bus.data.get();
        let e_in_instr = self.d_out_instr;

        println!(
            "-[${:X}]-> F -[{:08X}]-> D -[{:08X}]-> E",
            self.regs[PC], d_in_instr, e_in_instr
        );

        // Fetch stage
        println!("Fetching from PC={:X}", self.regs[PC]);
        if let Some(request) = self.bus_operation_for_state(self.current_execute_state) {
            bus.make_request(request);
        }

        // Decode stage
        self.d_out_instr = d_in_instr;

        // Execute stage
        let current_state = self.current_execute_state;
        self.current_execute_state = self.step_execute_fsm(bus, current_state, e_in_instr);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::pin::Pin;

    fn step(
        cpu: &mut ArmCpu,
        bus: &Bus,
        cycle_type: char,
        operation: char,
        bits: i32,
        address: u32,
        val: u32,
    ) {
        let seq = match cycle_type {
            'N' => false,
            'S' => true,
            x => panic!("Invalid cycle_type: {}", x),
        };

        let op = match operation {
            'R' => OperationType::Read {
                is_instruction: false,
            },
            'W' => OperationType::Write,
            'O' => OperationType::Read {
                is_instruction: true,
            },
            x => panic!("Invalid operation type: {}", x),
        };

        let width = match bits {
            8 => AccessWidth::Bit8,
            16 => AccessWidth::Bit16,
            32 => AccessWidth::Bit32,
            x => panic!("Invalid width: {}", x),
        };

        cpu.step(&bus);
        assert_eq!(
            bus.request.get(),
            Some(MemoryRequest {
                address,
                width,
                op,
                seq,
            })
        );
        bus.data.set(val);
    }

    fn step_i(cpu: &mut ArmCpu, bus: &Bus, cycle_type: char) {
        match cycle_type {
            'I' => (),
            x => panic!("Invalid cycle_type: {}", x),
        };

        cpu.step(&bus);
        assert_eq!(bus.request.get(), None);
    }

    #[test]
    fn test_mov() {
        let bus = Default::default();
        let mut cpu = ArmCpu::new();

        // mov r0, #0x0800'0000
        step(&mut cpu, &bus, 'N', 'O', 32, 0x00000000, 0xE3A00302);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000004, 0xFFFFFFFF);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000008, 0xFFFFFFFF);
        assert_eq!(cpu.regs[0], 0x0800_0000);
    }

    #[test]
    fn test_branch() {
        let bus = Default::default();
        let mut cpu = ArmCpu::new();

        // b loc_0020
        step(&mut cpu, &bus, 'N', 'O', 32, 0x00000000, 0xEA000006);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000004, 0xFFFFFFFF);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000008, 0xFFFFFFFF);
        // mov r0, #0x0800'0000
        step(&mut cpu, &bus, 'N', 'O', 32, 0x00000020, 0xE3A00302);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000024, 0xFFFFFFFF);
    }
}
