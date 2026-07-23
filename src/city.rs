//! Shared logical city names and provider-native search values.

use crate::{BossError, Platform};

struct CityCodes {
    name: &'static str,
    zhipin: &'static str,
    zhilian: &'static str,
    qiancheng: &'static str,
}

const CITIES: &[CityCodes] = &[
    CityCodes {
        name: "北京",
        zhipin: "101010100",
        zhilian: "530",
        qiancheng: "010000",
    },
    CityCodes {
        name: "上海",
        zhipin: "101020100",
        zhilian: "538",
        qiancheng: "020000",
    },
    CityCodes {
        name: "广州",
        zhipin: "101280100",
        zhilian: "763",
        qiancheng: "030200",
    },
    CityCodes {
        name: "深圳",
        zhipin: "101280600",
        zhilian: "765",
        qiancheng: "040000",
    },
    CityCodes {
        name: "杭州",
        zhipin: "101210100",
        zhilian: "653",
        qiancheng: "080200",
    },
    CityCodes {
        name: "成都",
        zhipin: "101270100",
        zhilian: "801",
        qiancheng: "090200",
    },
    CityCodes {
        name: "武汉",
        zhipin: "101200100",
        zhilian: "736",
        qiancheng: "180200",
    },
    CityCodes {
        name: "南京",
        zhipin: "101190100",
        zhilian: "635",
        qiancheng: "070200",
    },
    CityCodes {
        name: "苏州",
        zhipin: "101190400",
        zhilian: "639",
        qiancheng: "070300",
    },
    CityCodes {
        name: "西安",
        zhipin: "101110100",
        zhilian: "854",
        qiancheng: "200200",
    },
];

/// Returns the exact logical cities mapped for all three providers.
#[must_use]
pub fn names() -> Vec<&'static str> {
    CITIES.iter().map(|city| city.name).collect()
}

/// Resolves a logical Chinese city name to a provider-native value.
///
/// Numeric values pass through unchanged for explicit single-provider use.
pub fn provider_value(platform: Platform, city: &str) -> Result<&str, BossError> {
    if city.chars().all(|character| character.is_ascii_digit()) {
        return Ok(city);
    }
    CITIES
        .iter()
        .find(|codes| codes.name == city)
        .map(|codes| match platform {
            Platform::Zhipin => codes.zhipin,
            Platform::Zhilian => codes.zhilian,
            Platform::Qiancheng => codes.qiancheng,
        })
        .ok_or_else(|| BossError::InvalidArgument(format!("unsupported city: {city}")))
}

/// Validates a city before any provider request is attempted.
///
/// Provider-native numeric values are accepted only for a selected platform.
pub fn validate_selection(platform: Option<Platform>, city: &str) -> Result<(), BossError> {
    if city.chars().all(|character| character.is_ascii_digit()) {
        return if platform.is_some() {
            Ok(())
        } else {
            Err(BossError::InvalidArgument(
                "native numeric city requires a single platform".to_owned(),
            ))
        };
    }
    provider_value(platform.unwrap_or(Platform::Zhipin), city).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shenzhen_resolves_to_distinct_provider_values() {
        assert_eq!(
            [
                provider_value(Platform::Zhipin, "深圳").expect("zhipin"),
                provider_value(Platform::Zhilian, "深圳").expect("zhilian"),
                provider_value(Platform::Qiancheng, "深圳").expect("qiancheng"),
            ],
            ["101280600", "765", "040000"]
        );
    }

    #[test]
    fn native_numeric_city_requires_one_platform() {
        assert!(validate_selection(Some(Platform::Zhipin), "101280600").is_ok());
        assert!(validate_selection(None, "101280600").is_err());
    }

    #[test]
    fn common_city_count_is_exactly_ten() {
        assert_eq!(names().len(), 10);
    }
}
