// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{Addr, MainboardController, Reg};
use bitfield::bitfield;
use drv_fpga_api::{FpgaError, FpgaUserDesign, WriteOp};
use userlib::FromPrimitive;
use zerocopy::{AsBytes, FromBytes};

bitfield! {
    #[derive(Copy, Clone, PartialEq, Eq, FromPrimitive, AsBytes, FromBytes)]
    #[repr(C)]
    pub struct FanModuleState(u8);
    pub enable, set_enable: 0;
    pub led, set_led: 1;
    pub present, _: 2;
    pub power_good, _: 3;
    pub power_fault, _: 4;
    pub power_timed_out, _: 5;
}

/// Each fan module contains two fans and the SP applies control at the
/// individual fan level. Power control and status, module presence, and module
/// LED control exist at the module level.
pub enum FanModuleIndex {
    Zero = 0,
    One = 1,
    Two = 2,
    Three = 3,
}

impl From<u8> for FanModuleIndex {
    fn from(v: u8) -> Self {
        match v {
            0 => FanModuleIndex::Zero,
            1 => FanModuleIndex::One,
            2 => FanModuleIndex::Two,
            3 => FanModuleIndex::Three,
            _ => panic!(), // invalid fan module index
        }
    }
}

pub struct FanModules {
    fpga: FpgaUserDesign,
}

impl FanModules {
    pub fn new(task_id: userlib::TaskId) -> Self {
        Self {
            fpga: FpgaUserDesign::new(
                task_id,
                MainboardController::DEVICE_INDEX,
            ),
        }
    }

    pub fn state(&self) -> Result<[FanModuleState; 4], FpgaError> {
        self.fpga.read(Addr::FAN0_STATE)
    }

    pub fn set_enable(&self, idx: FanModuleIndex) -> Result<(), FpgaError> {
        self.fpga.write(
            WriteOp::BitSet,
            Addr::FAN0_STATE as u16 + idx as u16,
            Reg::FAN0_STATE::ENABLE,
        )?;

        Ok(())
    }

    pub fn clear_enable(&self, idx: FanModuleIndex) -> Result<(), FpgaError> {
        self.fpga.write(
            WriteOp::BitClear,
            Addr::FAN0_STATE as u16 + idx as u16,
            Reg::FAN0_STATE::ENABLE,
        )?;

        Ok(())
    }

    pub fn set_led(&self, idx: FanModuleIndex) -> Result<(), FpgaError> {
        self.fpga.write(
            WriteOp::BitSet,
            Addr::FAN0_STATE as u16 + idx as u16,
            Reg::FAN0_STATE::LED,
        )?;

        Ok(())
    }

    pub fn clear_led(&self, idx: FanModuleIndex) -> Result<(), FpgaError> {
        self.fpga.write(
            WriteOp::BitClear,
            Addr::FAN0_STATE as u16 + idx as u16,
            Reg::FAN0_STATE::LED,
        )?;

        Ok(())
    }
}
