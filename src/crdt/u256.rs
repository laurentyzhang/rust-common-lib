use alloy_primitives::U256 as AlloyU256;

use super::crdt::{CacheableCrdt, Crdt};
use super::state::Error;

#[derive(Clone, PartialEq)]
pub struct U256 {
    pub(crate) value: Option<AlloyU256>,
    pub(crate) delta: Option<AlloyU256>,
    pub(crate) limits: Option<(AlloyU256, AlloyU256)>,
}

impl Default for U256 {
    fn default() -> Self {
        Self {
            value: Some(AlloyU256::ZERO),
            delta: None,
            limits: Some((AlloyU256::ZERO, AlloyU256::MAX)),
        }
    }
}

impl U256 {
    fn check_limits(upper: AlloyU256, lower: AlloyU256, value: AlloyU256) -> Result<(), Error> {
        if lower > upper {
            return Err(Error::U256("lower limit must be less than upper limit"));
        }

        if value < lower {
            return Err(Error::U256("value is below the configured lower limit"));
        }

        if value > upper {
            return Err(Error::U256("value is above the configured upper limit"));
        }
        Ok(())
    }

    pub fn new(&mut self, upper: AlloyU256, lower: AlloyU256) -> Result<Self, Error> {
        Self::check_limits(upper, lower, AlloyU256::ZERO)?;

        self.limits = Some((AlloyU256::from(lower), AlloyU256::from(upper)));
        Ok(Self::default())
    }
}

impl Crdt<AlloyU256, AlloyU256> for U256 {
    type Error = Error;

    fn value(&self) -> Option<&AlloyU256> {
        self.value.as_ref()
    }

    fn delta(&self) -> Option<&AlloyU256> {
        self.delta.as_ref()
    }

    fn add_delta(&mut self, delta: &AlloyU256) -> Result<&AlloyU256, Self::Error> {
        let old_delta = self.delta;

        let accumulated = old_delta
            .unwrap_or(AlloyU256::ZERO)
            .checked_add(*delta)
            .ok_or(Error::U256("U256 overflow"))?;

        let projected = match self.value {
            Some(value) => value
                .checked_add(accumulated)
                .ok_or(Error::U256("U256 overflow"))?,
            None => accumulated,
        };

        if let Some((lower, upper)) = self.limits {
            if projected < lower {
                return Err(Error::U256("value is below the configured lower limit"));
            }

            if projected > upper {
                return Err(Error::U256("value is above the configured upper limit"));
            }
        }

        let stored = self.delta.insert(accumulated);
        Ok(&*stored)
    }

    fn apply_delta(&mut self) -> &Self {
        let Some(delta) = self.delta else {
            return self;
        };

        self.value = Some(self.value.unwrap_or(AlloyU256::ZERO) + delta);
        self.delta = None;
        self
    }

    fn limits(&self) -> Option<(&AlloyU256, &AlloyU256)> {
        self.limits.as_ref().map(|(lower, upper)| (lower, upper))
    }

    fn is_numeric(&self) -> bool {
        true
    }

    fn is_commutative(&self) -> bool {
        true
    }
}

impl CacheableCrdt<AlloyU256, AlloyU256> for U256 {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
