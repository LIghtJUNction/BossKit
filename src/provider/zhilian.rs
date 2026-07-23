use async_trait::async_trait;
use reqwest::header::{ACCEPT, COOKIE, ORIGIN, REFERER};
use serde_json::Value;

use super::{
    JobProvider, SearchRequest, cookie, first_text, overlay_list, overlay_text, parse_url,
    required_url, send_json, stable_id, text_list,
};
use crate::{BossError, Job, Platform};

/// 智联招聘 read-only search adapter.
pub struct ZhilianProvider {
    client: reqwest::Client,
}

impl ZhilianProvider {
    /// Creates the adapter.
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Builds the public search endpoint URL.
    pub fn search_url(request: &SearchRequest<'_>) -> Result<reqwest::Url, BossError> {
        let mut url = parse_url("https://fe-api.zhaopin.com/c/i/sou")?;
        let start = (request.page - 1) * request.limit;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("start", &start.to_string())
                .append_pair("pageSize", &request.limit.to_string())
                .append_pair("kw", request.query);
            if let Some(city) = request.city {
                pairs.append_pair(
                    "cityId",
                    crate::city::provider_value(Platform::Zhilian, city)?,
                );
            }
        }
        Ok(url)
    }

    fn parse(value: &Value) -> Result<Vec<Job>, BossError> {
        let items = value
            .pointer("/data/results")
            .and_then(Value::as_array)
            .ok_or_else(|| BossError::Parse("missing data.results".to_owned()))?;
        items.iter().map(Self::parse_job).collect()
    }

    fn parse_job(value: &Value) -> Result<Job, BossError> {
        let url = required_url(
            value.get("positionURL").and_then(Value::as_str),
            "positionURL",
        )?;
        let remote_id = value
            .get("number")
            .and_then(Value::as_str)
            .unwrap_or(&url)
            .to_owned();
        let title = required_url(value.get("jobName").and_then(Value::as_str), "jobName")?;
        let mut job = Job::new(
            stable_id(Platform::Zhilian, &remote_id, &url),
            Platform::Zhilian,
            remote_id,
            title,
            url,
        );
        job.company = first_text(value, &["/company/name", "/companyName"]);
        job.city = first_text(value, &["/city/display", "/cityName"]);
        job.district = first_text(value, &["/businessArea", "/district/name"]);
        job.salary = text(value, "salary");
        job.experience = first_text(value, &["/workingExp/name", "/experience"]);
        job.education = first_text(value, &["/eduLevel/name", "/education"]);
        job.employment_type = first_text(value, &["/emplType", "/jobType"]);
        job.skills = text_list(value, &["/skillLabel", "/skills"]);
        job.welfare = text_list(value, &["/welfare", "/welfareLabel"]);
        Ok(job)
    }

    /// Builds a path-segment-safe read-only detail URL.
    pub fn detail_url(job: &Job) -> Result<reqwest::Url, BossError> {
        let mut url = parse_url("https://fe-api.zhaopin.com/api/c/jobs/")?;
        url.path_segments_mut()
            .map_err(|_| BossError::InvalidArgument("invalid detail base URL".to_owned()))?
            .pop_if_empty()
            .push(&job.remote_id)
            .push("info");
        Ok(url)
    }

    fn parse_detail(base: &Job, value: &Value) -> Result<Job, BossError> {
        let data = value.get("data").unwrap_or(value);
        let mut job = base.clone();
        overlay_text(
            &mut job.title,
            first_text(data, &["/position/name", "/job/name", "/jobName", "/name"]),
        );
        overlay_text(
            &mut job.company,
            first_text(data, &["/company/name", "/companyName"]),
        );
        overlay_text(
            &mut job.city,
            first_text(data, &["/position/city/name", "/city/name", "/cityName"]),
        );
        overlay_text(
            &mut job.district,
            first_text(
                data,
                &["/position/district/name", "/district/name", "/district"],
            ),
        );
        overlay_text(
            &mut job.salary,
            first_text(data, &["/position/salary", "/salary"]),
        );
        overlay_text(
            &mut job.experience,
            first_text(
                data,
                &["/position/experience", "/experience/name", "/experience"],
            ),
        );
        overlay_text(
            &mut job.education,
            first_text(
                data,
                &["/position/education", "/education/name", "/education"],
            ),
        );
        overlay_text(
            &mut job.employment_type,
            first_text(data, &["/position/employmentType", "/jobType", "/emplType"]),
        );
        overlay_text(
            &mut job.description,
            first_text(
                data,
                &["/position/description", "/job/description", "/description"],
            ),
        );
        overlay_text(
            &mut job.address,
            first_text(data, &["/position/address", "/address"]),
        );
        overlay_list(
            &mut job.skills,
            text_list(data, &["/position/skills", "/skills", "/skillLabel"]),
        );
        overlay_list(
            &mut job.welfare,
            text_list(data, &["/position/welfare", "/welfare", "/welfareLabel"]),
        );
        if job.description.is_empty() {
            return Err(BossError::Parse(
                "智联 detail contained no job description".to_owned(),
            ));
        }
        Ok(job)
    }
}

#[async_trait]
impl JobProvider for ZhilianProvider {
    fn platform(&self) -> Platform {
        Platform::Zhilian
    }

    async fn search(&self, request: &SearchRequest<'_>) -> Result<Vec<Job>, BossError> {
        let mut builder = self
            .client
            .get(Self::search_url(request)?)
            .header(ACCEPT, "application/json")
            .header(ORIGIN, "https://sou.zhaopin.com")
            .header(REFERER, "https://sou.zhaopin.com/");
        if let Some(value) = cookie("BOSS_ZHILIAN_COOKIE") {
            builder = builder.header(COOKIE, value);
        }
        Self::parse(&send_json(builder).await?)
    }

    async fn detail(&self, job: &Job) -> Result<Job, BossError> {
        let mut builder = self
            .client
            .get(Self::detail_url(job)?)
            .header(ACCEPT, "application/json")
            .header(ORIGIN, "https://sou.zhaopin.com")
            .header(REFERER, &job.url);
        if let Some(value) = cookie("BOSS_ZHILIAN_COOKIE") {
            builder = builder.header(COOKIE, value);
        }
        Self::parse_detail(job, &send_json(builder).await?)
    }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_and_response_are_normalized() {
        let request = SearchRequest {
            query: "rust",
            city: Some("深圳"),
            page: 2,
            limit: 20,
        };
        let url = ZhilianProvider::search_url(&request).expect("url");
        assert!(url.as_str().contains("start=20"));
        assert!(url.as_str().contains("cityId=765"));
        let value =
            serde_json::from_str(include_str!("fixtures/zhilian_search.json")).expect("fixture");
        let jobs = ZhilianProvider::parse(&value).expect("parse");
        assert_eq!(
            (&jobs[0].company, &jobs[0].education, jobs[0].skills.len()),
            (&"Example Cloud".to_owned(), &"本科".to_owned(), 2)
        );
    }

    #[test]
    fn detail_url_escapes_path_and_overlay_parses_nested_data() {
        let base = Job::new("id", Platform::Zhilian, "../unsafe", "Old", "https://job");
        let url = ZhilianProvider::detail_url(&base).expect("detail url");
        let value =
            serde_json::from_str(include_str!("fixtures/zhilian_detail.json")).expect("fixture");
        let detail = ZhilianProvider::parse_detail(&base, &value).expect("detail");
        let invalid = ZhilianProvider::parse_detail(&base, &serde_json::json!({"data":{}}));
        assert!(
            url.path().contains("..%2Funsafe")
                && detail.description == "Design and operate reliable backend services."
                && invalid.is_err()
        );
    }
}
