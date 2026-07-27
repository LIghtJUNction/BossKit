use async_trait::async_trait;
use reqwest::header::{ACCEPT, COOKIE, REFERER};
use serde_json::Value;

use super::{
    JobProvider, SearchRequest, first_text, overlay_list, overlay_text, parse_url, required_url,
    send_json, stable_id, text_list,
};
use crate::{BossError, Job, Platform};

/// BOSS 直聘 read-only search adapter.
pub struct ZhipinProvider {
    client: reqwest::Client,
    cookie: Option<String>,
}

impl ZhipinProvider {
    /// Creates the adapter.
    #[must_use]
    pub fn new(client: reqwest::Client, cookie: Option<String>) -> Self {
        Self { client, cookie }
    }

    /// Builds the public search endpoint URL.
    pub fn search_url(request: &SearchRequest<'_>) -> Result<reqwest::Url, BossError> {
        let mut url = parse_url("https://www.zhipin.com/wapi/zpgeek/search/joblist.json")?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("scene", "1")
                .append_pair("query", request.query)
                .append_pair("page", &request.page.to_string())
                .append_pair("pageSize", &request.limit.to_string());
            if let Some(city) = request.city {
                pairs.append_pair("city", crate::city::provider_value(Platform::Zhipin, city)?);
            }
        }
        Ok(url)
    }

    fn parse(value: &Value) -> Result<Vec<Job>, BossError> {
        let items = value
            .pointer("/zpData/jobList")
            .and_then(Value::as_array)
            .ok_or_else(|| BossError::Parse("missing zpData.jobList".to_owned()))?;
        items.iter().map(Self::parse_job).collect()
    }

    fn parse_job(value: &Value) -> Result<Job, BossError> {
        let remote_id = required_url(
            value.get("encryptJobId").and_then(Value::as_str),
            "encryptJobId",
        )?;
        let url = format!("https://www.zhipin.com/job_detail/{remote_id}.html");
        let title = required_url(value.get("jobName").and_then(Value::as_str), "jobName")?;
        let mut job = Job::new(
            stable_id(Platform::Zhipin, &remote_id, &url),
            Platform::Zhipin,
            remote_id,
            title,
            url,
        );
        job.company = text(value, "brandName");
        job.city = text(value, "cityName");
        job.district = first_text(value, &["/areaDistrict", "/districtName"]);
        job.salary = text(value, "salaryDesc");
        job.experience = first_text(value, &["/jobExperience", "/experienceName"]);
        job.education = first_text(value, &["/jobDegree", "/degreeName"]);
        job.employment_type = first_text(value, &["/jobType", "/jobTypeName"]);
        job.skills = text_list(value, &["/skills", "/skillsList"]);
        job.welfare = text_list(value, &["/welfareList", "/welfare"]);
        Ok(job)
    }

    /// Builds the read-only detail endpoint.
    pub fn detail_url(job: &Job) -> Result<reqwest::Url, BossError> {
        let mut url = parse_url("https://www.zhipin.com/wapi/zpgeek/job/detail.json")?;
        url.query_pairs_mut()
            .append_pair("encryptJobId", &job.remote_id);
        Ok(url)
    }

    fn parse_detail(base: &Job, value: &Value) -> Result<Job, BossError> {
        let data = value
            .pointer("/zpData")
            .ok_or_else(|| BossError::Parse("missing zpData detail".to_owned()))?;
        let mut job = base.clone();
        overlay_text(
            &mut job.title,
            first_text(data, &["/jobInfo/jobName", "/jobInfo/name"]),
        );
        overlay_text(
            &mut job.company,
            first_text(
                data,
                &[
                    "/brandComInfo/brandName",
                    "/jobInfo/brandName",
                    "/bossInfo/brandName",
                ],
            ),
        );
        overlay_text(
            &mut job.city,
            first_text(data, &["/jobInfo/cityName", "/jobInfo/locationName"]),
        );
        overlay_text(
            &mut job.district,
            first_text(data, &["/jobInfo/areaDistrict", "/jobInfo/districtName"]),
        );
        overlay_text(
            &mut job.salary,
            first_text(data, &["/jobInfo/salaryDesc", "/jobInfo/salary"]),
        );
        overlay_text(
            &mut job.experience,
            first_text(data, &["/jobInfo/experienceName", "/jobInfo/jobExperience"]),
        );
        overlay_text(
            &mut job.education,
            first_text(data, &["/jobInfo/degreeName", "/jobInfo/jobDegree"]),
        );
        overlay_text(
            &mut job.employment_type,
            first_text(data, &["/jobInfo/jobTypeName", "/jobInfo/jobType"]),
        );
        overlay_text(
            &mut job.description,
            first_text(
                data,
                &[
                    "/jobDetail",
                    "/jobDetail/content",
                    "/jobDetail/jobDescription",
                    "/jobInfo/postDescription",
                ],
            ),
        );
        overlay_text(
            &mut job.address,
            first_text(data, &["/jobInfo/address", "/jobInfo/businessDistrict"]),
        );
        overlay_list(
            &mut job.skills,
            text_list(data, &["/jobInfo/skills", "/jobInfo/skillsList"]),
        );
        overlay_list(
            &mut job.welfare,
            text_list(data, &["/jobInfo/welfareList", "/jobInfo/welfare"]),
        );
        if job.description.is_empty() {
            return Err(BossError::Parse(
                "BOSS detail contained no job description".to_owned(),
            ));
        }
        Ok(job)
    }
}

#[async_trait]
impl JobProvider for ZhipinProvider {
    fn platform(&self) -> Platform {
        Platform::Zhipin
    }

    async fn search(&self, request: &SearchRequest<'_>) -> Result<Vec<Job>, BossError> {
        let mut builder = self
            .client
            .get(Self::search_url(request)?)
            .header(ACCEPT, "application/json")
            .header(REFERER, "https://www.zhipin.com/web/geek/job");
        if let Some(value) = self.cookie.as_deref() {
            builder = builder.header(COOKIE, value);
        }
        Self::parse(&send_json(builder).await?)
    }

    async fn detail(&self, job: &Job) -> Result<Job, BossError> {
        let mut builder = self
            .client
            .get(Self::detail_url(job)?)
            .header(ACCEPT, "application/json")
            .header(REFERER, &job.url);
        if let Some(value) = self.cookie.as_deref() {
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
            page: 1,
            limit: 10,
        };
        let url = ZhipinProvider::search_url(&request).expect("url");
        assert_eq!(url.path(), "/wapi/zpgeek/search/joblist.json");
        assert!(url.as_str().contains("query=rust"));
        assert!(url.as_str().contains("city=101280600"));
        let value =
            serde_json::from_str(include_str!("fixtures/zhipin_search.json")).expect("fixture");
        let jobs = ZhipinProvider::parse(&value).expect("parse");
        assert_eq!(
            (
                &jobs[0].remote_id,
                &jobs[0].experience,
                jobs[0].welfare.len()
            ),
            (&"sanitized-zp-001".to_owned(), &"3-5年".to_owned(), 2)
        );
    }

    #[test]
    fn detail_url_and_payload_overlay_cached_job() {
        let base = Job::new("id", Platform::Zhipin, "a/b", "Old", "https://job");
        let url = ZhipinProvider::detail_url(&base).expect("detail url");
        let value =
            serde_json::from_str(include_str!("fixtures/zhipin_detail.json")).expect("fixture");
        let detail = ZhipinProvider::parse_detail(&base, &value).expect("detail");
        let invalid = ZhipinProvider::parse_detail(&base, &serde_json::json!({"zpData":{}}));
        assert!(
            url.as_str().contains("encryptJobId=a%2Fb")
                && detail.description == "Build reliable Linux platform services in Rust."
                && invalid.is_err()
        );
    }
}
