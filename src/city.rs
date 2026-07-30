//! BOSS 直聘 logical city names and provider-native search values.

use crate::{BossError, Platform};

struct CityCodes {
    name: &'static str,
    zhipin: &'static str,
}

const CITIES: &[CityCodes] = &[
    CityCodes {
        name: "北京",
        zhipin: "101010100",
    },
    CityCodes {
        name: "上海",
        zhipin: "101020100",
    },
    CityCodes {
        name: "广州",
        zhipin: "101280100",
    },
    CityCodes {
        name: "深圳",
        zhipin: "101280600",
    },
    CityCodes {
        name: "杭州",
        zhipin: "101210100",
    },
    CityCodes {
        name: "成都",
        zhipin: "101270100",
    },
    CityCodes {
        name: "武汉",
        zhipin: "101200100",
    },
    CityCodes {
        name: "南京",
        zhipin: "101190100",
    },
    CityCodes {
        name: "苏州",
        zhipin: "101190400",
    },
    CityCodes {
        name: "西安",
        zhipin: "101110100",
    },
];

/// Returns the logical cities mapped for BOSS 直聘.
#[must_use]
pub fn names() -> Vec<&'static str> {
    CITIES.iter().map(|city| city.name).collect()
}

/// Resolves a logical Chinese city name to a provider-native value.
///
/// Numeric values pass through unchanged.
pub fn provider_value(_platform: Platform, city: &str) -> Result<&str, BossError> {
    if city.chars().all(|character| character.is_ascii_digit()) {
        return Ok(city);
    }
    CITIES
        .iter()
        .find(|codes| codes.name == city)
        .map(|codes| codes.zhipin)
        .ok_or_else(|| BossError::InvalidArgument(format!("unsupported city: {city}")))
}

/// Validates a city before any provider request is attempted.
///
pub fn validate_selection(city: &str) -> Result<(), BossError> {
    provider_value(Platform::Zhipin, city).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shenzhen_resolves_to_boss_value() {
        assert_eq!(
            provider_value(Platform::Zhipin, "深圳").expect("zhipin"),
            "101280600"
        );
    }

    #[test]
    fn native_numeric_city_is_valid() {
        assert!(validate_selection("101280600").is_ok());
    }

    #[test]
    fn common_city_count_is_exactly_ten() {
        assert_eq!(names().len(), 10);
    }
}
