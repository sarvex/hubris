// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use abi::{ImageHeader, ImageVectors};
use lpc55_romapi::FLASH_PAGE_SIZE;
use sha3::{Digest, Sha3_256};
use stage0_handoff::{ImageVersion, RotImageDetails};
use unwrap_lite::UnwrapLite;

pub fn get_image_b() -> Option<Image> {
    let imageb = unsafe { &__IMAGE_B_BASE };

    let img = Image(imageb);

    if img.validate() {
        Some(img)
    } else {
        None
    }
}

pub fn get_image_a() -> Option<Image> {
    let imagea = unsafe { &__IMAGE_A_BASE };

    let img = Image(imagea);

    if img.validate() {
        Some(img)
    } else {
        None
    }
}

extern "C" {
    static __IMAGE_A_BASE: abi::ImageVectors;
    static __IMAGE_B_BASE: abi::ImageVectors;
    // __vector size is currently defined in the linker script as
    //
    // __vector_size = SIZEOF(.vector_table);
    //
    // which is a symbol whose value is the size of the vector table (i.e.
    // there is no actual space allocated). This is best represented as a zero
    // sized type which gets accessed by addr_of! as below.
    static __vector_size: [u8; 0];
}

// FLASH_PAGE_SIZE is a usize so redefine the constant here to avoid having
// to do the u32 change everywhere
const PAGE_SIZE: u32 = FLASH_PAGE_SIZE as u32;

pub struct Image(&'static ImageVectors);

pub fn image_details(img: Image) -> RotImageDetails {
    RotImageDetails {
        digest: img.get_hash(),
        version: img.get_image_version(),
    }
}

impl Image {
    fn get_img_start(&self) -> u32 {
        self.0 as *const ImageVectors as u32
    }

    fn get_img_size(&self) -> Option<usize> {
        usize::try_from((unsafe { &*self.get_header() }).total_image_len).ok()
    }

    fn get_header(&self) -> *const ImageHeader {
        // SAFETY: This generated by the linker script which we trust
        // Note that this is generated from _this_ image's linker script
        // as opposed to the _image_ linker script but those two _must_
        // be the same value!
        let vector_size = unsafe { core::ptr::addr_of!(__vector_size) as u32 };
        (self.get_img_start() + vector_size) as *const ImageHeader
    }

    /// Make sure all of the image flash is programmed
    fn validate(&self) -> bool {
        let img_start = self.get_img_start();

        // Start by making sure we can access the page where the vectors live
        let valid = lpc55_romapi::validate_programmed(img_start, PAGE_SIZE);

        if !valid {
            return false;
        }

        let header_ptr = self.get_header();

        // Next validate the header location is programmed
        let valid =
            lpc55_romapi::validate_programmed(header_ptr as u32, PAGE_SIZE);

        if !valid {
            return false;
        }

        // SAFETY: We've validated the header location is programmed so this
        // will not trigger a fault. This is generated from our build scripts
        // which we trust.
        let header = unsafe { &*header_ptr };

        // Next make sure the marked image length is programmed
        let valid = lpc55_romapi::validate_programmed(
            img_start,
            (header.total_image_len + (PAGE_SIZE - 1)) & !(PAGE_SIZE - 1),
        );

        if !valid {
            return false;
        }

        // Does this look correct?
        if header.magic != abi::HEADER_MAGIC {
            return false;
        }

        return true;
    }

    // TODO: This is a particularly naive way to calculate the hash of the
    // hubris image: https://github.com/oxidecomputer/hubris/issues/736
    pub fn get_hash(&self) -> [u8; 32] {
        let img_ptr = self.get_img_start() as *const u8;
        // The MPU requires 32 byte alignment and so the compiler pads the
        // image accordingly. The length field from the image header does not
        // (and should not) account for this padding so we must do that here.
        let img_size = self.get_img_size().unwrap_lite() + 31 & !31;
        let image = unsafe { core::slice::from_raw_parts(img_ptr, img_size) };

        let mut img_hash = Sha3_256::new();
        img_hash.update(image);

        img_hash.finalize().try_into().unwrap_lite()
    }

    pub fn get_image_version(&self) -> ImageVersion {
        // SAFETY: We checked this previously
        let header = unsafe { &*self.get_header() };

        ImageVersion {
            epoch: header.epoch,
            version: header.version,
        }
    }

    fn pointer_range(&self) -> core::ops::Range<*const u8> {
        let img_ptr = self.get_img_start() as *const u8;
        // The MPU requires 32 byte alignment and so the compiler pads the
        // image accordingly. The length field from the image header does not
        // (and should not) account for this padding so we must do that here.
        let img_size = self.get_img_size().unwrap_lite() + 31 & !31;

        // Safety: this is unsafe because the pointer addition could overflow.
        // If that happens, we'll produce an empty range or crash with a panic.
        // We do not dereference these here pointers.
        img_ptr..unsafe { img_ptr.add(img_size) }
    }

    pub fn contains(&self, address: *const u8) -> bool {
        self.pointer_range().contains(&address)
    }
}
