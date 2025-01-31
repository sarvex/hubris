// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A driver for the LPC55 HighSpeed SPI interface.
//!
//! See drv/sprot-api/README.md
//! Messages are received from the Service Processor (SP) over a SPI interface.
//!
//! Only one request from the SP or one reply from the RoT will be handled
//! inside the io loop. This pattern does allow for potential pipelining of up
//! to 2 requests from the SP, with no changes on the RoT. Currently, however,
//! in the happy path, the SP will only send one request, wait for ROT_IRQ,
//! to be asserted by the RoT, and then clock in a response while clocking
//! out zeros. In this common case, the RoT will be clocking out zeros while
//! clocking in the request from the SP.
//!
//! See drv/sprot-api for message layout.
//!
//! If the payload length exceeds the maximum size or not all bytes are received
//! before CSn is de-asserted, the message is malformed and an ErrorRsp message
//! will be sent to the SP in the next message exchange.
//!
//! Messages from the SP are not processed until the SPI chip-select signal
//! is deasserted.
//!
//! ROT_IRQ is intended to be an edge triggered interrupt on the SP.
//! TODO: ROT_IRQ is currently sampled by the SP.
//! ROT_IRQ is de-asserted only after CSn is deasserted.
//!
//! TODO: SP RESET needs to be monitored, otherwise, any
//! forced looping here could be a denial of service attack against
//! observation of SP resetting. SP resetting without invalidating
//! security related state means a compromised SP could operate using
//! the trust gained in the previous session.
//! Upper layers may mitigate that, but check on it.

#![no_std]
#![no_main]

use drv_lpc55_gpio_api::{Direction, Value};
use drv_lpc55_spi as spi_core;
use drv_lpc55_syscon_api::{Peripheral, Syscon};
use drv_sprot_api::{
    RotIoStats, SprotProtocolError, REQUEST_BUF_SIZE, RESPONSE_BUF_SIZE,
    ROT_FIFO_SIZE,
};
use lpc55_pac as device;
use ringbuf::{ringbuf, ringbuf_entry};
use userlib::{
    sys_irq_control, sys_recv_closed, task_slot, TaskId, UnwrapLite,
};

mod handler;

use handler::Handler;

#[derive(Copy, Clone, PartialEq)]
pub(crate) enum Trace {
    None,
    Dump(u32),
    ReceivedBytes(usize),
    SentBytes(usize),
    Flush,
    FlowError,
    ReplyLen(usize),
    Underrun,
    Err(SprotProtocolError),
    Stats(RotIoStats),
}
ringbuf!(Trace, 32, Trace::None);

task_slot!(SYSCON, syscon_driver);
task_slot!(GPIO, gpio_driver);

/// Setup spi and its associated GPIO pins
fn configure_spi() -> Io {
    let syscon = Syscon::from(SYSCON.get_task_id());

    // Turn the actual peripheral on so that we can interact with it.
    turn_on_flexcomm(&syscon);

    let gpio_driver = GPIO.get_task_id();
    setup_pins(gpio_driver).unwrap_lite();
    let gpio = drv_lpc55_gpio_api::Pins::from(gpio_driver);

    // Configure ROT_IRQ
    // Ensure that ROT_IRQ is not asserted
    gpio.set_dir(ROT_IRQ, Direction::Output);
    gpio.set_val(ROT_IRQ, Value::One);

    // We have two blocks to worry about: the FLEXCOMM for switching
    // between modes and the actual SPI block. These are technically
    // part of the same block for the purposes of a register block
    // in app.toml but separate for the purposes of writing here

    let flexcomm = unsafe { &*device::FLEXCOMM8::ptr() };

    let registers = unsafe { &*device::SPI8::ptr() };

    let mut spi = spi_core::Spi::from(registers);

    // This should correspond to SPI mode 0
    spi.initialize(
        device::spi0::cfg::MASTER_A::SLAVE_MODE,
        device::spi0::cfg::LSBF_A::STANDARD, // MSB First
        device::spi0::cfg::CPHA_A::CHANGE,
        device::spi0::cfg::CPOL_A::LOW,
        spi_core::TxLvl::Tx7Items,
        spi_core::RxLvl::Rx1Item,
    );
    // Set SPI mode for Flexcomm
    flexcomm.pselid.write(|w| w.persel().spi());

    // Drain and configure FIFOs
    spi.enable();

    // We only want interrupts on CSn assert
    // Once we see that interrupt we enter polling mode
    // and check the registers manually.
    spi.ssa_enable();

    // Probably not necessary, drain Rx and Tx after config.
    spi.drain();

    // Disable the interrupts triggered by the `self.spi.drain_tx()`, which
    // unneccessarily causes spurious interrupts. We really only need to to
    // respond to CSn asserted interrupts, because after that we always enter a
    // tight loop.
    spi.disable_tx();
    spi.disable_rx();

    Io {
        spi,
        gpio,
        stats: RotIoStats::default(),
        rot_irq_asserted: false,
    }
}

// Container for spi and gpio
struct Io {
    spi: crate::spi_core::Spi,
    gpio: drv_lpc55_gpio_api::Pins,
    stats: RotIoStats,

    /// This is an optimization to avoid talking to the GPIO task when we don't
    /// have to.
    /// ROT_IRQ is deasserted on startup in main.
    rot_irq_asserted: bool,
}

enum IoError {
    Flush,
    Flow,
}

#[export_name = "main"]
fn main() -> ! {
    let mut io = configure_spi();

    let (rx_buf, tx_buf) = mutable_statics::mutable_statics! {
        static mut RX_BUF: [u8; REQUEST_BUF_SIZE] = [|| 0; _];
        static mut TX_BUF: [u8; RESPONSE_BUF_SIZE] = [|| 0; _];
    };

    let mut handler = Handler::new();

    // Prime our write fifo, so we clock out zero bytes on the next receive
    io.spi.drain_tx();
    while io.spi.can_tx() {
        io.spi.send_u16(0);
    }

    loop {
        let rsp_len = match io.wait_for_request(rx_buf) {
            Ok(rx_len) => {
                handler.handle(&rx_buf[..rx_len], tx_buf, &mut io.stats)
            }
            Err(IoError::Flush) => {
                // A flush indicates that the server should de-assert ROT_IRQ
                // as instructed by the SP. We do that and then proceed to wait
                // for the next request.
                ringbuf_entry!(Trace::Flush);
                io.deassert_rot_irq();
                continue;
            }
            Err(IoError::Flow) => {
                ringbuf_entry!(Trace::FlowError);
                handler.flow_error(tx_buf)
            }
        };

        ringbuf_entry!(Trace::Stats(io.stats));
        io.reply(&tx_buf[..rsp_len]);
    }
}

impl Io {
    // Wait for chip select to be asserted
    // Assert ROT_IRQ if this is a reply
    fn wait_for_csn_asserted(&mut self, is_reply: bool) {
        loop {
            sys_irq_control(notifications::SPI_IRQ_MASK, true);

            if is_reply && !self.rot_irq_asserted {
                self.assert_rot_irq();
            }

            sys_recv_closed(
                &mut [],
                notifications::SPI_IRQ_MASK,
                TaskId::KERNEL,
            )
            .unwrap_lite();

            // Is CSn asserted by the SP?
            let intstat = self.spi.intstat();
            if intstat.ssa().bit() {
                self.spi.ssa_clear();
                break;
            }
        }
    }

    pub fn wait_for_request(
        &mut self,
        rx_buf: &mut [u8],
    ) -> Result<usize, IoError> {
        self.wait_for_csn_asserted(false);

        // Go into a tight loop receiving as many bytes as we can until we see
        // CSn de-asserted.
        let mut bytes_received = 0;
        let mut rx = rx_buf.iter_mut();
        while !self.spi.ssd() {
            while self.spi.has_entry() {
                bytes_received += 2;
                let read = self.spi.read_u16();
                let upper = (read >> 8) as u8;
                let lower = read as u8;
                rx.next().map(|b| *b = upper);
                rx.next().map(|b| *b = lower);
            }
        }

        self.spi.ssd_clear();

        // There may be bytes left in the rx fifo after CSn is de-asserted
        while self.spi.has_entry() {
            bytes_received += 2;
            let read = self.spi.read_u16();
            let upper = (read >> 8) as u8;
            let lower = read as u8;
            rx.next().map(|b| *b = upper);
            rx.next().map(|b| *b = lower);
        }

        self.check_for_rx_error()?;

        if bytes_received == 0 {
            // This was a CSn pulse
            self.stats.csn_pulses = self.stats.csn_pulses.wrapping_add(1);
            return Err(IoError::Flush);
        }

        ringbuf_entry!(Trace::ReceivedBytes(bytes_received));

        // We don't bother sending bytes when receiving. So we must clear
        // the underrun error condition before we handle a reply.
        self.spi.txerr_clear();

        Ok(bytes_received)
    }

    fn reply(&mut self, tx_buf: &[u8]) {
        ringbuf_entry!(Trace::ReplyLen(tx_buf.len()));

        let mut idx = 0;

        // Fill in the fifo before we assert ROT_IRQ
        // We assert ROT_IRQ in `wait_for_csn_asserted` so we can
        // put it after the `sys_irq_control` syscall to minimize time taken to
        // process a request.
        self.spi.drain_tx();
        while self.spi.can_tx() {
            let entry = get_u16(idx, tx_buf);
            self.spi.send_u16(entry);
            idx += 2;
        }

        self.wait_for_csn_asserted(true);

        while !self.spi.ssd() {
            while self.spi.can_tx() {
                let entry = get_u16(idx, tx_buf);
                self.spi.send_u16(entry);
                idx += 2;
            }
        }

        self.spi.ssd_clear();

        // Were any bytes clocked out?
        // We check to see if any bytes in the fifo have been sent or any have
        // been pushed into the fifo beyond the initial fill.
        if !self.spi.can_tx() && idx == ROT_FIFO_SIZE {
            // This was a CSn pulse
            // There's no need to flush here, since we de-assert ROT_IRQ at the
            // bottom of this function, which is the purpose of a flush.
            self.stats.csn_pulses = self.stats.csn_pulses.wrapping_add(1);
        } else {
            self.check_for_tx_error();
        }

        ringbuf_entry!(Trace::SentBytes(idx - ROT_FIFO_SIZE));

        // Prime our write fifo, so we clock out zero bytes on the next receive
        // We also empty our read fifo, since we don't bother reading bytes while writing.
        self.spi.drain();
        while self.spi.can_tx() {
            self.spi.send_u16(0);
        }
        // We don't bother receiving bytes when sending. So we must clear
        // the overrun error condition for the next time we wait for a reply.
        self.spi.rxerr_clear();

        // Now that we are ready to handle the next request, let the SP know we
        // are ready.
        self.deassert_rot_irq();
    }

    fn check_for_rx_error(&mut self) -> Result<(), IoError> {
        let fifostat = self.spi.fifostat();
        if fifostat.rxerr().bit() {
            self.spi.rxerr_clear();
            self.stats.rx_overrun = self.stats.rx_overrun.wrapping_add(1);
            Err(IoError::Flow)
        } else {
            Ok(())
        }
    }

    // We don't actually want to return an error here.
    // The SP will detect an underrun via a CRC error
    fn check_for_tx_error(&mut self) {
        let fifostat = self.spi.fifostat();

        if fifostat.txerr().bit() {
            // We don't do anything with tx errors other than record them
            // The SP will see a checksum error if this is a reply, or the
            // underrun happened after the number of reply bytes and it
            // doesn't matter.
            self.spi.txerr_clear();
            self.stats.tx_underrun = self.stats.tx_underrun.wrapping_add(1);
            ringbuf_entry!(Trace::Underrun);
        }
    }

    fn assert_rot_irq(&mut self) {
        self.gpio.set_val(ROT_IRQ, Value::Zero);
        self.rot_irq_asserted = true;
    }

    fn deassert_rot_irq(&mut self) {
        self.gpio.set_val(ROT_IRQ, Value::One);
        self.rot_irq_asserted = false;
    }
}

// Return 2 bytes starting at `idx` combined into a u16 for putting on a fifo
// If `idx` >= `tx_buf.len()` use 0 for the byte.
fn get_u16(idx: usize, tx_buf: &[u8]) -> u16 {
    let upper = tx_buf.get(idx).copied().unwrap_or(0) as u16;
    let lower = tx_buf.get(idx + 1).copied().unwrap_or(0) as u16;
    upper << 8 | lower
}

fn turn_on_flexcomm(syscon: &Syscon) {
    // HSLSPI = High Speed Spi = Flexcomm 8
    // The L stands for Let this just be named consistently for once
    syscon.enable_clock(Peripheral::HsLspi);
    syscon.leave_reset(Peripheral::HsLspi);
}

include!(concat!(env!("OUT_DIR"), "/pin_config.rs"));
include!(concat!(env!("OUT_DIR"), "/notifications.rs"));
