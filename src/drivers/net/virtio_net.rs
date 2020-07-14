// Copyright (c) 2020 Frederik Schulz, RWTH Aachen University
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! A module containing a virtio network driver.
//! 
//! The module contains ...
use arch::x86_64::kernel::pci::PciAdapter;
use arch::x86_64::kernel::pci::error::PciError;
use core::result::Result;
use alloc::vec::Vec;
use core::mem;

use drivers::virtio::env::memory::{MemLen, MemOff};
use drivers::virtio::transport::pci::{UniCapsColl, ComCfg, ShMemCfg, NotifCfg, IsrStatus, PciCfgAlt, PciCap};
use drivers::virtio::transport::pci;
use drivers::virtio::driver::VirtioDriver;
use drivers::virtio::error::VirtioError;
use drivers::virtio::virtqueue::Virtq;
use drivers::virtio::virtqueue::packed::PackedVq;
use drivers::virtio::virtqueue::split::SplitVq;

use self::error::VirtioNetError;
use self::constants::{Features, Status};
/// Virtio's network device feature bits
/// See Virtio specficiation v1.1. - 5.1.3
#[allow(dead_code, non_camel_case_types)]
#[repr(u32)]
pub enum NetFeatures {
    VIRTIO_NET_F_CSUM= 0,
    VIRTIO_NET_F_GUEST_CSUM = 1,
    VIRTIO_NET_F_CTRL_GUEST_OFFLOADS = 2,
    VIRTIO_NET_F_MTU = 3,
    VIRTIO_NET_F_MAC = 5,
    VIRTIO_NET_F_GUEST_TSO4 = 7,
    VIRTIO_NET_F_GUEST_TSO6 = 8,
    VIRTIO_NET_F_GUEST_ECN = 9,
    VIRTIO_NET_F_GUEST_UFO = 10,
    VIRTIO_NET_F_HOST_TSO4 = 11,
    VIRTIO_NET_F_HOST_TSO6 = 12,
    VIRTIO_NET_F_HOST_ECN = 13,
    VIRTIO_NET_F_HOST_UFO = 14,
    VIRTIO_NET_F_MRG_RXBUF = 15,
    VIRTIO_NET_F_STATUS = 16,
    VIRTIO_NET_F_CTRL_VQ = 17,
    VIRTIO_NET_F_CTRL_RX = 18,
    VIRTIO_NET_F_CTRL_VLAN = 19,
    VIRTIO_NET_F_CTRL_RX_EXTRA = 20,
    VIRTIO_NET_F_GUEST_ANNOUNCE = 21,
    VIRTIO_NET_F_MQ = 22,
    VIRTIO_NET_F_CTRL_MAC_ADDR = 23,
    VIRTIO_NET_F_GSO = 6,
}

/// A wrapper struct for the raw configuration structure. 
/// Handling the right access to fields, as some are read-only
/// for the driver.
///
/// 
pub struct NetDevCfg {
    raw: &'static NetDevCfgRaw,
    dev_id: u16,

    // Feature booleans

}

/// Virtio's network device configuration structure. 
/// See specification v1.1. - 5.1.4
///
#[repr(C)]
struct NetDevCfgRaw {
	mac: [u8; 6],
	status: u16,
	max_virtqueue_pairs: u16,
	mtu: u16,
}

pub struct VirtioNetDriver {
    dev_cfg: NetDevCfg,
    com_cfg: ComCfg,
    isr_stat: IsrStatus,
    notif_cfg: NotifCfg,
}

impl VirtioDriver for VirtioNetDriver {
    fn add_buff(&self) {
        unimplemented!();
    }

    fn get_buff(&self) {
        unimplemented!();
    }

    fn process_buff(&self) {
        unimplemented!();
    }

    fn set_notif(&self){
        unimplemented!();
    }
}

// Private funtctions for Virtio network driver
impl VirtioNetDriver {
    fn map_cfg(cap: &PciCap) -> Option<NetDevCfg> {
        if cap.bar_len() <  u64::from(cap.len() + cap.offset()) {
            error!("Network config of device {:x}, does not fit into memeory specified by bar!", 
                cap.dev_id(),
            );
            return None
        }

        // Drivers MAY do this check. See Virtio specification v1.1. - 4.1.4.1
        if cap.len() < MemLen::from(mem::size_of::<NetDevCfg>()*8) {
            error!("Network config from device {:x}, does not represent actual structure specified by the standard!", cap.dev_id());
            return None 
        }

        let virt_addr_raw = cap.bar_addr() + cap.offset();

        // Create mutable reference to the PCI structure in PCI memory
        let dev_cfg: &mut NetDevCfgRaw = unsafe {
            &mut *(usize::from(virt_addr_raw) as *mut NetDevCfgRaw)
        };

        Some(NetDevCfg {
            raw: dev_cfg,
            dev_id: cap.dev_id()
        })
    }

    /// Instanciates a new (VirtioNetDriver)[VirtioNetDriver] struct, by checking the available 
    /// configuration structures and moving them into the struct.
    fn new(mut caps_coll: UniCapsColl, adapter: &PciAdapter) -> Result<Self, error::VirtioNetError> {
        let com_cfg =  loop { 
            match caps_coll.get_com_cfg() {
                Some(com_cfg) => break com_cfg,
                None => return Err(error::VirtioNetError::NoComCfg(adapter.device_id)),
            }
        };

        let isr_stat = loop {
            match caps_coll.get_isr_cfg(){
                Some(isr_stat) => break isr_stat,
                None => return Err(error::VirtioNetError::NoIsrCfg(adapter.device_id)),
            }
        };

        let notif_cfg = loop {
            match caps_coll.get_notif_cfg() {
                Some(notif_cfg) => break notif_cfg,
                None => return Err(error::VirtioNetError::NoNotifCfg(adapter.device_id)),
            }
        };

        let dev_cfg = loop {
            match caps_coll.get_dev_cfg() {
                Some(cfg) => { 
                    match VirtioNetDriver::map_cfg(&cfg) {
                        Some(dev_cfg) => break dev_cfg,
                        None => (),
                    }
                },
                None => return Err(error::VirtioNetError::NoDevCfg(adapter.device_id)),
            }
        };

        Ok(VirtioNetDriver {
            dev_cfg,
            com_cfg,
            isr_stat,
            notif_cfg
        })
    }

    /// Initallizes the device in adherence to specificaton. Returns Some(VirtioNetError)
    /// upon failure and None in case everything worked as expected.
    ///
    /// See Virtio specification v1.1. - 3.1.1. 
    ///                      and v1.1. - 5.1.5
    fn init_dev(&mut self) -> Result<(), VirtioNetError> {
        self.com_cfg.reset_dev();
        self.com_cfg.ack_dev();
        self.com_cfg.set_drv();

        match self.negotiate_features() {
            Ok(_) => info!("Driver found a subset of features for virtio device {:x}.", self.dev_cfg.dev_id),
            Err(vnet_err) => return Err(vnet_err),
        }
        
        self.com_cfg.features_ok();

        if self.com_cfg.check_features() {
            info!("Features have been negotiated between network device {:x} and driver.", self.dev_cfg.dev_id);
            // If wanted, one could renegotiate features here!
        } else {
            return Err(VirtioNetError::FailFeatureNeg(self.dev_cfg.dev_id));
        }

        match self.dev_spec_init() {
            Ok(_) => info!("Device specific initalization for Virtio network defice {:x} finished", self.dev_cfg.dev_id),
            Err(vnet_err) => return Err(vnet_err),
        }

        // At this point the device is "live"
        self.com_cfg.drv_ok();

        Ok(())
    }

    /// Negotiates a subset of features both understood and wanted by both the OS 
    /// and the device.
    fn negotiate_features(&mut self) -> Result<(), VirtioNetError> {
        let dev_feats = self.com_cfg.dev_features();

        let required_feats: u64 = Features::VIRTIO_NET_F_MAC
            | Features::VIRTIO_NET_F_STATUS
            | Features::VIRTIO_NET_F_GUEST_UFO
            | Features::VIRTIO_NET_F_GUEST_TSO4
            | Features::VIRTIO_NET_F_GUEST_TSO6
            | Features::VIRTIO_NET_F_GUEST_CSUM;

        if dev_feats & required_feats == required_feats {
            self.com_cfg.set_drv_features(required_feats);
            Ok(())
        } else {
            Err(VirtioNetError::FailFeatureNeg(self.dev_cfg.dev_id))
        }
    }

    /// Device Specfic initalization according to Virtio specifictation v1.1. - 5.1.5
    fn dev_spec_init(&mut self) -> Result<(), VirtioNetError> {
        todo!();
    }
}

// Public interface for virtio network driver.
impl VirtioNetDriver { 
    /// Initializes virtio network device by mapping configuration layout to 
    /// respective structs (configuration structs are:
    /// [ComCfg](structs.comcfg.html), [NotifCfg](structs.notifcfg.html)
    /// [IsrStatus](structs.isrstatus.html), [PciCfg](structs.pcicfg.html)
    /// [ShMemCfg](structs.ShMemCfg)). 
    ///
    /// Returns a driver instance of 
    /// [VirtioNetDriver](structs.virtionetdriver.html) or an [VirtioError](enums.virtioerror.html).
    pub fn init(adapter: &PciAdapter) -> Result<VirtioNetDriver, VirtioError> {
        let mut drv = match pci::map_caps(adapter) {
            Ok(caps) => match VirtioNetDriver::new(caps, adapter) {
                Ok(driver) => driver,
                Err(vnet_err) => return Err(VirtioError::NetDriver(vnet_err)),
            },
            Err(pci_error) => return Err(VirtioError::FromPci(pci_error)),
        };

        match drv.init_dev() {
            Ok(_) => info!("Network device with id {:x}, has been initalized by driver!", drv.dev_cfg.dev_id),
            Err(vnet_err) => {
                drv.com_cfg.set_failed();
                return Err(VirtioError::NetDriver(vnet_err))
            },
        }

        if drv.dev_status() & u16::from(Status::VIRTIO_NET_S_LINK_UP) == u16::from(Status::VIRTIO_NET_S_LINK_UP) {
            info!("Virtio-net link is up after initalization.")
        } else {
            info!("Virtio-net link is down after initalization!")
        }

        Ok(drv)
    }

    pub fn dev_status(&self) -> u16 {
        self.dev_cfg.raw.status
    }
}

mod constants {
    use core::ops::BitOr;
    /// Enum contains virtio's network device features
    ///
    /// See Virtio specification v1.1. - 5.1.3
    #[allow(dead_code, non_camel_case_types)]
    #[derive(Copy, Clone, Debug)]
    #[repr(u64)]
    pub enum Features {
        VIRTIO_NET_F_CSUM = 0,
        VIRTIO_NET_F_GUEST_CSUM = 1 << 0,
        VIRTIO_NET_F_CTRL_GUEST_OFFLOADS = 1 << 1,
        VIRTIO_NET_F_MTU = 1 << 2, 
        VIRTIO_NET_F_MAC = 1 << 4,
        VIRTIO_NET_F_GUEST_TSO4 = 1 << 6,
        VIRTIO_NET_F_GUEST_TSO6 = 1 << 7,
        VIRTIO_NET_F_GUEST_ECN = 1 <<  8,
        VIRTIO_NET_F_GUEST_UFO = 1 <<  9,
        VIRTIO_NET_F_HOST_TSO4 = 1 <<  10,
        VIRTIO_NET_F_HOST_TSO6 = 1 <<  11,
        VIRTIO_NET_F_HOST_ECN = 1 <<  12,
        VIRTIO_NET_F_HOST_UFO = 1 <<  13,
        VIRTIO_NET_F_MRG_RXBUF = 1 <<  14,
        VIRTIO_NET_F_STATUS = 1 <<  15,
        VIRTIO_NET_F_CTRL_VQ = 1 <<  16,
        VIRTIO_NET_F_CTRL_RX = 1 <<  17,
        VIRTIO_NET_F_CTRL_VLAN = 1 << 18,
        VIRTIO_NET_F_GUEST_ANNOUNCE = 1 << 20,
        VIRTIO_NET_F_MQ = 1 << 21,
        VIRTIO_NET_F_CTRL_MAC_ADDR = 1 << 22,
        VIRTIO_NET_F_GUEST_HDRLEN = 1 << 59,
        VIRTIO_NET_F_RSC_EXT = 1 << 60,
        VIRTIO_NET_F_STANDBY = 1 << 61,

        // 5.1.3.1 Feature bit requirements
        // Some networking feature bits require other networking feature bits (see 2.2.1): VIRTIO_NET_F_GUEST_TSO4 Requires VIRTIO_NET_F_GUEST_CSUM.
        // VIRTIO_NET_F_GUEST_TSO6 Requires VIRTIO_NET_F_GUEST_CSUM.
        // VIRTIO_NET_F_GUEST_ECN RequiresVIRTIO_NET_F_GUEST_TSO4orVIRTIO_NET_F_GUEST_TSO6. VIRTIO_NET_F_GUEST_UFO Requires VIRTIO_NET_F_GUEST_CSUM.
        // VIRTIO_NET_F_HOST_TSO4 Requires VIRTIO_NET_F_CSUM.
        // VIRTIO_NET_F_HOST_TSO6 Requires VIRTIO_NET_F_CSUM.
        // VIRTIO_NET_F_HOST_ECN Requires VIRTIO_NET_F_HOST_TSO4 or VIRTIO_NET_F_HOST_TSO6. VIRTIO_NET_F_HOST_UFO Requires VIRTIO_NET_F_CSUM.
        // VIRTIO_NET_F_CTRL_RX Requires VIRTIO_NET_F_CTRL_VQ.
        // VIRTIO_NET_F_CTRL_VLAN Requires VIRTIO_NET_F_CTRL_VQ. VIRTIO_NET_F_GUEST_ANNOUNCE Requires VIRTIO_NET_F_CTRL_VQ.
        // VIRTIO_NET_F_MQ Requires VIRTIO_NET_F_CTRL_VQ.
        // VIRTIO_NET_F_CTRL_MAC_ADDR Requires VIRTIO_NET_F_CTRL_VQ.
        // VIRTIO_NET_F_RSC_EXT Requires VIRTIO_NET_F_HOST_TSO4 or VIRTIO_NET_F_HOST_TSO6.
    }

    impl From<Features> for u64 {
        fn from(val: Features) -> Self {
           match val {
            Features::VIRTIO_NET_F_CSUM => 0,
            Features::VIRTIO_NET_F_GUEST_CSUM => 1 << 0,
            Features::VIRTIO_NET_F_CTRL_GUEST_OFFLOADS => 1 << 1,
            Features::VIRTIO_NET_F_MTU => 1 << 2, 
            Features::VIRTIO_NET_F_MAC => 1 << 4,
            Features::VIRTIO_NET_F_GUEST_TSO4 => 1 << 6,
            Features::VIRTIO_NET_F_GUEST_TSO6 => 1 << 7,
            Features::VIRTIO_NET_F_GUEST_ECN => 1 <<  8,
            Features::VIRTIO_NET_F_GUEST_UFO => 1 <<  9,
            Features::VIRTIO_NET_F_HOST_TSO4 => 1 <<  10,
            Features::VIRTIO_NET_F_HOST_TSO6 => 1 <<  11,
            Features::VIRTIO_NET_F_HOST_ECN => 1 <<  12,
            Features::VIRTIO_NET_F_HOST_UFO => 1 <<  13,
            Features::VIRTIO_NET_F_MRG_RXBUF => 1 <<  14,
            Features::VIRTIO_NET_F_STATUS => 1 <<  15,
            Features::VIRTIO_NET_F_CTRL_VQ => 1 <<  16,
            Features::VIRTIO_NET_F_CTRL_RX => 1 <<  17,
            Features::VIRTIO_NET_F_CTRL_VLAN => 1 << 18,
            Features::VIRTIO_NET_F_GUEST_ANNOUNCE => 1 << 20,
            Features::VIRTIO_NET_F_MQ => 1 << 21,
            Features::VIRTIO_NET_F_CTRL_MAC_ADDR => 1 << 22,
            Features::VIRTIO_NET_F_GUEST_HDRLEN => 1 << 59,
            Features::VIRTIO_NET_F_RSC_EXT => 1 << 60,
            Features::VIRTIO_NET_F_STANDBY => 1 << 61,
           } 
        }
    }

    impl BitOr for Features {
        type Output = u64;

        fn bitor(self, rhs: Self) -> Self::Output {
           u64::from(self) | u64::from(rhs) 
        }
    }

    impl BitOr<Features> for u64 {
        type Output = u64;

        fn bitor(self, rhs: Features) -> Self::Output {
            self | u64::from(rhs)
        }
    }

    /// Enum contains virtio's network device status
    /// indiacted in the status field of the device's 
    /// configuration structure.
    ///
    /// See Virtio specification v1.1. - 5.1.4
    #[allow(dead_code, non_camel_case_types)]
    #[derive(Copy, Clone, Debug)]
    #[repr(u16)]
    pub enum Status {
        VIRTIO_NET_S_LINK_UP = 1 << 0,
        VIRTIO_NET_S_ANNOUNCE = 1 << 1,
    }

    impl From<Status> for u16 {
        fn from(stat: Status) -> Self {
            match stat {
                Status::VIRTIO_NET_S_LINK_UP => 1,
                Status::VIRTIO_NET_S_ANNOUNCE => 2,
            }
        }
    }
}

/// Error module of virtios network driver. Containing the (VirtioNetError)[VirtioNetError]
/// enum.
pub mod error {
    /// Network drivers error enum.
    #[derive(Debug, Copy, Clone)]
    pub enum VirtioNetError {
        General,
        NoDevCfg(u16),
        NoComCfg(u16),
        NoIsrCfg(u16),
        NoNotifCfg(u16),
        FailFeatureNeg(u16),
    }
}
