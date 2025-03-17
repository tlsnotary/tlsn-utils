#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]

/// Compatibility extension trait for RNGs implementing `0.9` `RngCore` trait.
pub trait Rand0_6CompatExt {
    /// Wraps `self` in a compatibility wrapper that implements `0.6` traits.
    fn compat(self) -> Rand0_6CompatWrapper<Self>
    where
        Self: Sized,
    {
        Rand0_6CompatWrapper(self)
    }

    /// Wraps `self` in a compatibility wrapper that implements `0.6` traits.
    ///
    /// Same as [`Rand0_6CompatExt::compat`] but instead of taking ownership it borrows.
    fn compat_by_ref(&mut self) -> Rand0_6CompatWrapper<&mut Self>
    where
        Self: Sized,
    {
        Rand0_6CompatWrapper(self)
    }
}

impl<T> Rand0_6CompatExt for T where T: rand_core::TryRngCore + ?Sized {}

/// Rand 0.6 compatibility wrapper.
pub struct Rand0_6CompatWrapper<R: ?Sized>(R);

impl<R> Rand0_6CompatWrapper<R> {
    /// Creates a new compatibility wrapper.
    pub fn new(inner: R) -> Self {
        Self(inner)
    }

    /// Returns the inner value.
    pub fn into_inner(self) -> R {
        self.0
    }
}

#[cfg(not(feature = "std"))]
impl<R> rand_core_06::RngCore for Rand0_6CompatWrapper<R>
where
    R: rand_core::TryRngCore + ?Sized,
{
    fn next_u32(&mut self) -> u32 {
        self.0.try_next_u32().unwrap()
    }

    fn next_u64(&mut self) -> u64 {
        self.0.try_next_u64().unwrap()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.try_fill_bytes(dest).unwrap()
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core_06::Error> {
        self.0
            .try_fill_bytes(dest)
            .map_err(|_| rand_core_06::Error::from(core::num::NonZeroU32::new(1).unwrap()))
    }
}

#[cfg(feature = "std")]
impl<R> rand_core_06::RngCore for Rand0_6CompatWrapper<R>
where
    R: rand_core::TryRngCore + ?Sized,
    R::Error: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
{
    fn next_u32(&mut self) -> u32 {
        self.0.try_next_u32().unwrap()
    }

    fn next_u64(&mut self) -> u64 {
        self.0.try_next_u64().unwrap()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.try_fill_bytes(dest).unwrap()
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core_06::Error> {
        self.0
            .try_fill_bytes(dest)
            .map_err(rand_core_06::Error::new)
    }
}

impl<R> rand_core_06::CryptoRng for Rand0_6CompatWrapper<R> where R: rand_core::CryptoRng + ?Sized {}

#[cfg(test)]
mod tests {
    use super::*;

    use rand_core::OsRng;
    use rand_core_06::RngCore as RngCore06;

    fn foo<R>(rng: &mut R)
    where
        R: RngCore06,
    {
        let _: u32 = rng.next_u32();
    }

    #[test]
    fn test_compat_os_rng() {
        foo(&mut OsRng.compat());
    }

    #[test]
    fn test_compat_thread_rng() {
        let mut rng = rand::rng();
        foo(&mut (rng.compat_by_ref()));
        foo(&mut rng.compat());
    }
}
