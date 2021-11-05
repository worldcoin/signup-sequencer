use std::hash::Hasher;

use crypto::{
    digest::Digest,
    sha3::{Sha3, Sha3Mode},
};
use merkletree::hash::Algorithm;

pub struct ExampleAlgorithm(Sha3);

// TODO implement MiMC and various optimizations
impl ExampleAlgorithm {
    pub fn new() -> ExampleAlgorithm {
        ExampleAlgorithm(Sha3::new(Sha3Mode::Sha3_256))
    }
}

impl Default for ExampleAlgorithm {
    fn default() -> ExampleAlgorithm {
        ExampleAlgorithm::new()
    }
}

impl Hasher for ExampleAlgorithm {
    #[inline]
    fn write(&mut self, msg: &[u8]) {
        self.0.input(msg)
    }

    #[inline]
    fn finish(&self) -> u64 {
        unimplemented!()
    }
}

impl Algorithm<[u8; 32]> for ExampleAlgorithm {
    #[inline]
    fn hash(&mut self) -> [u8; 32] {
        let mut h = [0u8; 32];
        self.0.result(&mut h);
        h
    }

    #[inline]
    fn reset(&mut self) {
        self.0.reset();
    }
}

pub struct MiMCAlgorithm {}

impl MiMCAlgorithm {
    pub fn new() -> MiMCAlgorithm {
        MiMCAlgorithm {}
    }
}

impl Default for MiMCAlgorithm {
    fn default() -> MiMCAlgorithm {
        MiMCAlgorithm::new()
    }
}

impl Hasher for MiMCAlgorithm {
    #[inline]
    fn write(&mut self, _msg: &[u8]) {
        // TODO
        unimplemented!()
    }

    #[inline]
    fn finish(&self) -> u64 {
        unimplemented!()
    }
}

impl Algorithm<[u8; 32]> for MiMCAlgorithm {
    #[inline]
    fn hash(&mut self) -> [u8; 32] {
        // TODO
        unimplemented!()
    }

    #[inline]
    fn reset(&mut self) {
        // TODO
        unimplemented!()
    }
}

// hashLeftRight
// (R, C) = MiMC.MiMCSponge (left, 0, 0)
// R = addmod(R, right, SNARK_SCALAR_FIELD_CONSTANT)
// (R, C) = MiMC.MiMCSponge (R, C, 0)
// return R;
