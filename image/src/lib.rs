#![no_std]

use zerocopy::byteorder::{ByteOrder, LittleEndian};
use zerocopy::{AsBytes, FromBytes, LayoutVerified};
use hubpack::SerializedSize;
use serde::{Deserialize, Serialize};

pub fn add(left: usize, right: usize) -> usize {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}



// XXX This is a way, but not the recommended way to account for all the
// images that need validation and signing.
// TODO: It would be nice to have some generated code for each
// artifact type.
// TODO: This is not a comprehensive list if we include Gimletlets pressed
// into other roles where we want to sign images.
#[repr(C)]
#[derive(
    AsBytes,
    Eq,
    Copy,
    PartialEq,
    Clone,
    Serialize,
    Deserialize,
    SerializedSize,
)]
pub enum Artifact {
    Nonce64,
    _GimletletRotLpc55S69HubrisA,
    _GimletletRotLpc55S69HubrisB,
    GimletletRotLpc55S69Stage0,
    _GimletRotLpc55S69HubrisA,
    _GimletRotLpc55S69HubrisB,
    _GimletRotLpc55S69Stage0,
    _GimletSpStm32H53Hubris,
    _PscRotLpc55S69HubrisA,
    _PscRotLpc55S69HubrisB,
    _PscRotLpc55S69Stage0,
    _PscSpStm32H53Hubris,
    _SidecarRotLpc55S69HubrisA,
    _SidecarRotLpc55S69HubrisB,
    _SidecarRotLpc55S69Stage0,
    _SidecarSpStm32H53Hubris,
}

// XXX Get these all from an authoritative source.
const LPC55S69_FLASH_PAGE_SIZE: usize = 512;
const LPC55S69_MIN_SIZE: usize = 8 * 512;    // XXX Not the real number
const LPC55S69_MAX_SIZE: usize = 2000 * 512; // XXX Not the real number
const HEADER_MAGIC: u32 = 0x1535_6637; // XXX from hubris,sys/abi/src/lib.rs


#[derive(Copy, Clone, Debug, FromBytes, AsBytes, PartialEq)]
#[repr(C, packed)]
pub struct SAUEntry {
    pub rbar: u32,
    pub rlar: u32,
}

#[derive(Copy, Clone, Debug, FromBytes, AsBytes, PartialEq)]
#[repr(C, packed)]
pub struct ImageHeader {
    pub magic: u32,
    pub total_image_len: u32,
    pub sau_entries: [SAUEntry; 8],
    pub version: u32,
    pub epoch: u32,
}


/// What went wrong checking an artifact?
#[derive(Debug)]
pub enum ArtifactError {
    NotImplemented,
    Failed,
}

pub fn check_artifact(artifact: Artifact, content: &[u8]) -> Result<(), ArtifactError> {
    let mut passed = 0usize;
    let mut total = 0usize;
    // TODO: check content
    match artifact {
        Artifact::Nonce64 => {
            total += 1;
            if content.len() == (core::mem::size_of::<u64>()) {
                passed += 1;
            }

            // All zeros is not a nonce.
            total += 1;
            if !content.iter().all(|&b| b == 0) {
                passed += 1;
            }

            // All ones is not a nonce.
            total += 1;
            if !content.iter().all(|&b| b == 0xff) {
                passed += 1;
            }
        }
        Artifact::GimletletRotLpc55S69Stage0 => {
            if content.len() % LPC55S69_FLASH_PAGE_SIZE != 0 {
                return Err(ArtifactError::Failed);
            }
            total += 1;
            if (LPC55S69_MIN_SIZE..LPC55S69_MAX_SIZE).contains(&content.len()) {
                passed += 1;
            }

            let mut sp: Option<u32> = None;
            let mut pc: Option<u32> = None;
            let mut magic_offset: Option<usize> = None;

            for offset in (0..LPC55S69_FLASH_PAGE_SIZE).step_by(4) {
                if let Some(word) = content.get(offset..(offset + core::mem::size_of::<u32>())) .map(LittleEndian::read_u32) {

                    match offset {
                        0 => sp = Some(word), // SP
                        4 => pc = Some(word), // PC
                        _ => {
                            if word == HEADER_MAGIC {
                                magic_offset = Some(offset);
                                break;
                            }
                        },
                    }
                }
            }

            if sp.is_none() || sp.unwrap() == 0
                || pc.is_none() || pc.unwrap() == 0
                    || magic_offset.is_none() {

                        return Err(ArtifactError::Failed);
            }
            let header = ImageHeader::read_from_prefix(
                &content[magic_offset.ok_or(ArtifactError::Failed)?..])
                .ok_or(ArtifactError::Failed)?;
            if header.magic != HEADER_MAGIC
                || ((header.total_image_len as usize) > content.len())
                    || !header.sau_entries.into_iter().all(|e| e.rbar == 0 && e.rlar == 0)
                    || header.version == 0
                    || header.epoch == 0 {
                        return Err(ArtifactError::Failed)
                    } else {
                        total += 1;
                        passed += 1;
            }
        }
        _ => {
            // Not implemented
        }
    }

    if total == 0 {
        Err(ArtifactError::NotImplemented)
    } else if passed == total {
        Ok(())
    } else {
        Err(ArtifactError::Failed)
    }
}

pub fn find_le_magic(buf: &[u8], magic: u32) -> Option<usize> {
    for index in (0..buf.len()).step_by(core::mem::size_of::<u32>()) {
        if let Some(hbuf) = buf.get(index..(index+core::mem::size_of::<u32>())) {
            if magic == LittleEndian::read_u32(hbuf) {
                return Some(index)
            }
        } else {
            return None
        }
    }
    None
}
    

pub fn header(buf: &[u8]) -> Option<(usize, ImageHeader)> {
    if let Some(offset) = find_le_magic(buf, HEADER_MAGIC) {
        LayoutVerified::<_, ImageHeader>::new_from_prefix(&buf[offset..]).map(|(image, _remainder)| (offset, *image))
    } else {
        None
    }
}
