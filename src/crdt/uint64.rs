use super::crdt::{CacheableCrdt, Crdt};
use super::state::Error;

#[derive(Clone, PartialEq)]
pub struct U64 {
    pub(crate) value: Option<u64>,
    pub(crate) delta: Option<u64>,
    pub(crate) limits: Option<(u64, u64)>,
}

impl Default for U64 {
    fn default() -> Self {
        Self {
            value: Some(0),
            delta: None,
            limits: Some((u64::MIN, u64::MAX)),
        }
    }
}

impl U64 {
    fn check_limits(upper: u64, lower: u64, value: u64) -> Result<(), Error> {
        if lower > upper {
            return Err(Error::U64("lower limit must be less than upper limit"));
        }

        if value < lower {
            return Err(Error::U64("value is below the configured lower limit"));
        }

        if value > upper {
            return Err(Error::U64("value is above the configured upper limit"));
        }
        Ok(())
    }

    pub fn new(&mut self, upper: u64, lower: u64) -> Result<Self, Error> {
        Self::check_limits(upper, lower, 0)?;

        self.limits = Some((lower, upper));
        Ok(Self::default())
    }
}

impl Crdt<u64, u64> for U64 {
    type Error = Error;

    fn value(&self) -> Option<&u64> {
        self.value.as_ref()
    }

    fn delta(&self) -> Option<&u64> {
        self.delta.as_ref()
    }

    fn add_delta(&mut self, delta: &u64) -> Result<&u64, Error> {
        let old_delta = self.delta;
        let accumulated = old_delta
            .unwrap_or(0)
            .checked_add(*delta)
            .ok_or(Error::U64("u64 overflow"))?;
        let projected = match self.value {
            Some(value) => value
                .checked_add(accumulated)
                .ok_or(Error::U64("u64 overflow"))?,
            None => accumulated,
        };

        if let Some((_, upper)) = self.limits {
            if projected > upper {
                return Err(Error::U64("value is above the configured upper limit"));
            }
        }

        let stored = self.delta.insert(accumulated);
        Ok(&*stored)
    }

    fn apply_delta(&mut self) -> &Self {
        let Some(delta) = self.delta else {
            return self;
        };

        self.value = Some(self.value.unwrap_or(0) + delta);
        self.delta = None;
        self
    }

    fn limits(&self) -> Option<(&u64, &u64)> {
        self.limits.as_ref().map(|(lower, upper)| (lower, upper))
    }

    fn is_numeric(&self) -> bool {
        true
    }

    fn is_commutative(&self) -> bool {
        true
    }
}

impl CacheableCrdt<u64, u64> for U64 {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
