// Named constants for common registers
const LR: usize = 14;
const PC: usize = 15;

struct ArmCpu {
    regs: [u32; 16],
    cpsr: u32,
}

trait DecodeInstruction {
    fn decode_arm_instruction(instr: u32) -> Self;
}

//#[derive(DecodeInstruction)] TODO: Optimize with procedural macro later
#[derive(Debug, Eq, PartialEq)]
enum DecodedArmInstruction {
    DataProcessingImmediate {
        cond: u8,
        opcode: u8,
        s: bool,
        rn: u8,
        rd: u8,
        rotate: u8,
        imm: u8,
    },
    UndefinedInstruction,
    UnknownInstruction,
}

/// Tests instr against a bit pattern. Positions where format is '0' or '1' must have 0 or 1. Any
/// other character matches any bit, except for '_' which is skipped.
fn test(mut instr: u32, format: &'static [u8]) -> bool {
    assert_eq!(format.len(), 32 + 7);
    for c in format.iter().rev() {
        let bit = instr & 1;
        match c {
            b'0' if bit != 0 => return false,
            b'1' if bit != 1 => return false,
            b'_' => continue, // skip shifting instr
            _ => (),
        }
        instr >>= 1;
    }

    true
}

impl DecodeInstruction for DecodedArmInstruction {
    fn decode_arm_instruction(instr: u32) -> DecodedArmInstruction {
        use self::DecodedArmInstruction::*;

        if test(instr, b"cccc_001o_ooos_nnnn_dddd_rrrr_iiii_iiii") {
            return DataProcessingImmediate {
                cond: bit!(instr[28:31]) as u8,
                opcode: bit!(instr[21:24]) as u8,
                s: bit!(instr[20]) != 0,
                rn: bit!(instr[16:19]) as u8,
                rd: bit!(instr[12:15]) as u8,
                rotate: bit!(instr[8:11]) as u8,
                imm: bit!(instr[0:7]) as u8,
            };
        }

        UnknownInstruction
    }
}

mod tests {
    use super::*;

    #[test]
    fn test_decode_data_processing_imm() {
        let instr = 0xE3A00302; // mov r0, #134217728
        let actual = DecodedArmInstruction::decode_arm_instruction(instr);
        let expected = DecodedArmInstruction::DataProcessingImmediate {
            cond: 0b1110,
            opcode: 0b1101,
            s: false,
            rn: 0,
            rd: 0,
            rotate: 3,
            imm: 0x02,
        };
        assert_eq!(actual, expected);
    }
}
