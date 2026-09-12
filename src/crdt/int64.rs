use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::Error;

#[derive(Clone, PartialEq)]
pub struct I64 {
    pub(crate) value: Option<i64>,
    pub(crate) delta: Option<i64>,
    pub(crate) limits: Option<(i64, i64)>,
}

impl I64 {
    pub fn new(&mut self, upper: i64, lower: i64) -> Result<Self, Error> {
        Self::check_limits(upper, lower, 0)?;
        Ok(Self {
            value: Some(0),
            delta: None,
            limits: Some((lower, upper)),
        })
    }

    fn checked_add(left: i64, right: i64) -> Result<i64, Error> {
        left.checked_add(right).ok_or(if right < 0 {
            Error::I64("i64 underflow")
        } else {
            Error::I64("i64 overflow")
        })
    }

    fn check_limits(upper: i64, lower: i64, value: i64) -> Result<(), Error> {
        if value < lower {
            Err(Error::I64("value is below the configured lower limit"))
        } else if value > upper {
            Err(Error::I64("value is above the configured upper limit"))
        } else if upper < lower {
            Err(Error::I64("upper limit is below the lower limit"))
        } else {
            Ok(())
        }
    }
}

impl Default for I64 {
    fn default() -> Self {
        Self {
            value: Some(0),
            delta: None,
            limits: Some((i64::MIN, i64::MAX)),
        }
    }
}

impl Crdt<i64, i64> for I64 {
    type Error = Error;

    fn value(&self) -> Option<&i64> {
        self.value.as_ref()
    }

    fn delta(&self) -> Option<&i64> {
        self.delta.as_ref()
    }

    fn add_delta(&mut self, delta: &i64) -> Result<&i64, Self::Error> {
        let old_delta = self.delta;
        let accumulated = Self::checked_add(old_delta.unwrap_or(0), *delta)?;
        let projected = match self.value {
            Some(value) => Self::checked_add(value, accumulated)?,
            None => accumulated,
        };

        if let Some((lower, upper)) = self.limits {
            if projected < lower {
                return Err(Error::I64("value is below the configured lower limit"));
            }
            if projected > upper {
                return Err(Error::I64("value is above the configured upper limit"));
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

    fn limits(&self) -> Option<(&i64, &i64)> {
        self.limits.as_ref().map(|(lower, upper)| (lower, upper))
    }

    fn is_numeric(&self) -> bool {
        true
    }

    fn is_commutative(&self) -> bool {
        true
    }
}

impl CacheableCrdt<i64, i64> for I64 {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
